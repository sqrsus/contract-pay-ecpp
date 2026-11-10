//! 登录与权限：两种角色（浏览用户 / 管理员），密码以加盐 SHA-256 摘要存在 `sys_meta`。
//!
//! 设计约束（2026-11-06 确认）：
//! * **不新增表、不改表结构**：只在既有的 `sys_meta` 里写一行键值，存量数据库打开即用；
//! * 库里只存摘要，不存明文；摘要里带固定盐前缀，避免同一密码在不同库里得到相同串；
//! * 角色只影响界面内的写操作，**不是数据库级安全边界**（拿到 .db 用命令行仍可改）。
//!
//! 登录状态由界面层持有，只存在进程内：关闭程序即失效，下次启动重新登录。

use ctp_core::digest::sha256_hex;
use ctp_store::{db, StoreError};
use rusqlite::Connection;

/// 管理密码摘要所在的 `sys_meta` 键名。
pub const PASSWORD_META_KEY: &str = "auth.admin_password";

/// 首次使用时的初始管理密码。
pub const INITIAL_PASSWORD: &str = "0001";

/// 摘要盐前缀（改这个会让所有已设密码失效，不要动）。
const SALT: &str = "ctp-admin-v1:";

/// 新密码长度约束（位）。
pub const MIN_PASSWORD_LEN: usize = 4;
pub const MAX_PASSWORD_LEN: usize = 20;

/// 登录角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// 浏览用户：可看全部页面，不能改任何数据
    Browse,
    /// 管理员：全部权限
    Admin,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Browse => "浏览用户",
            Role::Admin => "管理员",
        }
    }

    /// 能否执行写操作。
    pub fn can_write(self) -> bool {
        self == Role::Admin
    }
}

/// 密码摘要：`sha256(盐 + 明文)` 的小写十六进制。
pub fn digest(password: &str) -> String {
    sha256_hex(format!("{SALT}{password}").as_bytes())
}

/// 库里是否已设置过管理密码（全新库为 `false`）。
pub fn password_set(conn: &Connection) -> Result<bool, StoreError> {
    Ok(db::get_meta(conn, PASSWORD_META_KEY)?.is_some())
}

/// 首次打开数据库时写入初始密码；已有密码则原样保留。
///
/// 返回 `true` 表示本次是新写入（首次初始化）。
pub fn ensure_initial(conn: &Connection) -> Result<bool, StoreError> {
    if password_set(conn)? {
        return Ok(false);
    }
    db::set_meta(conn, PASSWORD_META_KEY, &digest(INITIAL_PASSWORD), "system")?;
    Ok(true)
}

/// 校验密码。未初始化时按初始密码 `0001` 比对，保证首次使用不会把人挡在外面。
pub fn verify(conn: &Connection, input: &str) -> Result<bool, StoreError> {
    let stored = db::get_meta(conn, PASSWORD_META_KEY)?;
    let expected = stored.unwrap_or_else(|| digest(INITIAL_PASSWORD));
    Ok(expected == digest(input))
}

/// 当前管理密码是否仍是初始值（用于登录页提示「首次使用」）。
pub fn is_initial(conn: &Connection) -> Result<bool, StoreError> {
    match db::get_meta(conn, PASSWORD_META_KEY)? {
        Some(h) => Ok(h == digest(INITIAL_PASSWORD)),
        None => Ok(true),
    }
}

/// 改密码。调用方必须先 `verify` 旧密码，这里只负责写入。
///
/// R-31：同时写一条操作日志。日志里只记「已修改」，**不记任何密码痕迹**
/// （明文、摘要都不写）。
/// R-39：摘要与日志放进同一个 IMMEDIATE 事务，避免「密码改了但没留痕」。
pub fn change(conn: &mut Connection, new_password: &str, operator: &str) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    db::set_meta(&tx, PASSWORD_META_KEY, &digest(new_password), operator)?;
    db::insert_audit_log(
        &tx,
        "sys_meta",
        PASSWORD_META_KEY,
        Some("管理密码"),
        "CHANGE_PASSWORD",
        Some("管理密码已修改"),
        operator,
    )?;
    tx.commit()?;
    Ok(())
}

/// 记录一次管理登录尝试（成功 / 失败）。
///
/// 登录本身不写业务数据，但它是审计价值最高的一类事件，所以单独开这个入口。
/// 由界面层在密码校验后直接调用：此时角色尚未落定，不能走「写权限」那条门面。
pub fn log_login(conn: &Connection, ok: bool, operator: &str) -> Result<(), StoreError> {
    let (action, detail) = if ok {
        ("LOGIN_OK", "管理登录成功")
    } else {
        ("LOGIN_FAIL", "管理登录失败：密码错误")
    };
    db::insert_audit_log(
        conn,
        "session",
        "login",
        Some("管理登录"),
        action,
        Some(detail),
        operator,
    )
}

/// 新密码的格式校验：长度 4–20 位，不能含首尾空白，不能全是空白。
pub fn validate_new_password(p: &str) -> Result<(), String> {
    if p.trim().is_empty() {
        return Err("新密码不能为空".into());
    }
    if p.trim() != p {
        return Err("新密码首尾不能有空格".into());
    }
    let n = p.chars().count();
    if !(MIN_PASSWORD_LEN..=MAX_PASSWORD_LEN).contains(&n) {
        return Err(format!(
            "新密码需 {MIN_PASSWORD_LEN}–{MAX_PASSWORD_LEN} 位，当前 {n} 位"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let mut c = db::open_in_memory().unwrap();
        db::init_schema(&mut c).unwrap();
        c
    }

    #[test]
    fn 初始密码是0001且只写一次() {
        let mut c = conn();
        assert!(!password_set(&c).unwrap());
        assert!(ensure_initial(&c).unwrap(), "首次应写入");
        assert!(password_set(&c).unwrap());
        assert!(is_initial(&c).unwrap());

        // 再调一次不应覆盖用户已改的密码
        change(&mut c, "8866", "t").unwrap();
        assert!(!ensure_initial(&c).unwrap(), "已有密码时不应重写");
        assert!(verify(&c, "8866").unwrap());
        assert!(!verify(&c, "0001").unwrap());
        assert!(!is_initial(&c).unwrap());
    }

    #[test]
    fn 库里不存明文() {
        let c = conn();
        ensure_initial(&c).unwrap();
        let stored = db::get_meta(&c, PASSWORD_META_KEY).unwrap().unwrap();
        assert_eq!(stored.len(), 64, "应为 64 位十六进制摘要");
        assert!(!stored.contains("0001"));
    }

    #[test]
    fn 未初始化时按初始密码校验() {
        let c = conn();
        assert!(verify(&c, "0001").unwrap());
        assert!(!verify(&c, "0002").unwrap());
    }

    #[test]
    fn 新密码格式校验() {
        assert!(validate_new_password("0001").is_ok());
        assert!(validate_new_password("abcd1234").is_ok());
        assert!(validate_new_password("").is_err());
        assert!(validate_new_password("abc").is_err(), "少于 4 位应拒绝");
        assert!(
            validate_new_password(&"a".repeat(21)).is_err(),
            "超过 20 位应拒绝"
        );
        assert!(validate_new_password(" 1234").is_err(), "首部空格应拒绝");
    }

    #[test]
    fn 角色写权限() {
        assert!(Role::Admin.can_write());
        assert!(!Role::Browse.can_write());
        assert_eq!(Role::Browse.label(), "浏览用户");
    }
}
