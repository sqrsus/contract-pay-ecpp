use crate::error::{Result, StoreError};
use crate::schema;
use ctp_core::date::now_iso;
use ctp_core::rate::Rate;
use rusqlite::{Connection, OptionalExtension};

/// 打开文件库并完成 PRAGMA 设置（外键、WAL、忙等待）。
pub fn open(path: &str) -> Result<Connection> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StoreError::Inconsistent(format!("无法创建目录 {parent:?}：{e}")))?;
        }
    }
    let conn = Connection::open(path)?;
    configure(&conn)?;
    // WAL 只能对文件库设置；用 query_row 读取返回的 journal_mode，避免 execute 报「返回了结果」
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    Ok(conn)
}

/// 内存库（测试与自检用），不设 WAL。
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    // R-68（C5）：WAL + NORMAL 只保证「库不损坏」，不保证「最近已提交事务不丢」
    // （掉电时 WAL 里已提交但未 checkpoint 的帧可能丢失）。账目系统宁可多一次
    // fsync：FULL 让每次提交都落盘。单机单用户场景，千行导入多花的秒级时间可接受。
    conn.pragma_update(None, "synchronous", "FULL")?;
    // R-67：附件以 BLOB 入库，替换 / 删除时 SQLite 只把旧页标记为空闲，
    // 旧文件内容仍以明文留在文件里（默认 secure_delete = OFF），用十六进制工具就能捞出来。
    // 打开即清零，配合 `compact` 一起用：前者管「内容不残留」，后者管「文件缩回去」。
    conn.pragma_update(None, "secure_delete", "ON")?;
    Ok(())
}

/// 物理回收：把空闲页还给文件系统，并把 WAL 里的结果合并回主库。
///
/// 只写大 BLOB 的表做 `UPDATE` / `DELETE` 时，SQLite 把旧页挂到 freelist 上复用，
/// **文件高水位不会回落** —— 10 MB 附件换成 1 MB 后，库文件仍是 10 MB。
/// 只有 `VACUUM` 会重建整个库；而 WAL 模式下 `VACUUM` 的结果先是落在 WAL 里，
/// 必须再 `wal_checkpoint(TRUNCATE)`，主库文件的大小才会真的变小（漏了这步会以为没生效）。
///
/// 调用方负责把频率压住：只在附件体积确实变小时调用，别每次写都跑一遍。
pub fn compact(conn: &Connection) -> Result<()> {
    // VACUUM 不能在事务里执行，必须在业务事务提交之后调用
    conn.execute_batch("VACUUM")?;
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        // 返回 (busy, log, checkpointed)，这里只关心它跑完；内存库不会是 WAL
        let (_busy, _log, _done): (i64, i64, i64) =
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
    }
    Ok(())
}

/// 首次打开时建库：建表 + 建索引 + 建视图；重复执行幂等，**不删除任何已有数据**。
///
/// 本软件尚未正式投用，库内均为演示数据，因此不提供迁移功能：结构变更直接改 DDL，
/// 旧库由部署方删除后重新生成。这样读到的每一列都由当前 DDL 保证，
/// 不存在「旧形态 + 兼容分支」带来的口径分叉，也不存在启动时的隐式改写。
///
/// 与旧版 `migrate` 的区别：无版本守卫、无补列 / 删列、无枚举改写、无同名单位去重、
/// 无序号校准、无旧日志搬迁；`sys_meta` 里也不再写 `schema_version` / `tol_bp`。
pub fn init_schema(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (_, sql) in schema::table_statements() {
        tx.execute_batch(sql)?;
    }
    for sql in schema::index_statements() {
        tx.execute_batch(sql)?;
    }
    // 视图依赖表，且 v_payment_ledger 依赖 v_contract_summary；
    // view_statements() 的顺序已按依赖排好，直接顺序建即可。
    for (_, sql) in schema::view_statements() {
        tx.execute_batch(&sql)?;
    }
    tx.commit()?;
    Ok(())
}

pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let v = conn
        .query_row(
            "SELECT meta_value FROM sys_meta WHERE meta_key = ?1",
            [key],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    Ok(v)
}

pub fn set_meta(conn: &Connection, key: &str, value: &str, operator: &str) -> Result<()> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO sys_meta(meta_key, meta_value, created_at, created_by, updated_at, updated_by, version) \
         VALUES (?1, ?2, ?3, ?4, ?3, ?4, 1) \
         ON CONFLICT(meta_key) DO UPDATE SET \
            meta_value = excluded.meta_value, updated_at = excluded.updated_at, \
            updated_by = excluded.updated_by, version = sys_meta.version + 1",
        rusqlite::params![key, value, now, operator],
    )?;
    Ok(())
}

/// 超付容差（BR-15）：**业务口径固定为 0**，界面与命令行都没有配置入口。
///
/// 保留函数签名是为了让「容差」这一业务概念有唯一取数点；不读 `sys_meta`，
/// 也不会被任何写入影响。
pub fn tolerance(_conn: &Connection) -> Result<Rate> {
    Ok(Rate::ZERO)
}

/// 业务流水号：唯一入口，事务内自增（4.2.4 目标态写法，本期即用）。
pub fn next_sequence(conn: &Connection, scope_key: &str) -> Result<i64> {
    let now = now_iso();
    let v: i64 = conn.query_row(
        "INSERT INTO sys_sequence(scope_key, next_val, created_at, updated_at, version) \
         VALUES (?1, 1, ?2, ?2, 1) \
         ON CONFLICT(scope_key) DO UPDATE SET \
            next_val = sys_sequence.next_val + 1, updated_at = excluded.updated_at, \
            version = sys_sequence.version + 1 \
         RETURNING next_val",
        rusqlite::params![scope_key, now],
        |r| r.get(0),
    )?;
    Ok(v)
}

/// 日志保留上限（R-31）：超过就静默裁掉最旧的若干条，防止库无限膨胀。
///
/// 5 万条按每天 50 次操作算约三年，日常使用不会触发；触发时用户仍可在
/// 「操作日志」页看到最近的全部记录。
pub const AUDIT_KEEP_MAX: i64 = 50_000;

/// 把用户输入的关键词当**字面子串**处理，不允许 `%` / `_` 变成 LIKE 通配。
///
/// 必须与 SQL 里的 `ESCAPE '\'` 配套。日志清理与按项目汇总的关键词筛选用同一实现，
/// 避免「搜一个 % 就命中全表」把按条件清理伪装成全部清理。
pub fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 写一条操作日志。返回 `Err` **只表示这条日志没插进去**。
///
/// 裁剪（超过 5 万条删最旧）是下游的空间维护，失败不影响本条日志已落库的事实，
/// 且下次写入会重试；把它的错误也返回会让「批次审计日志失败」的告警误报。
pub fn insert_audit_log(
    conn: &Connection,
    target_type: &str,
    target_id: &str,
    target_label: Option<&str>,
    action: &str,
    detail: Option<&str>,
    operator_id: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sys_audit_log(id, target_type, target_id, target_label, action, detail, operator_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            ctp_core::ids::new_id("LOG"),
            target_type,
            target_id,
            target_label,
            action,
            detail,
            operator_id,
            now_iso()
        ],
    )?;
    let _ = prune_audit_log(conn);
    Ok(())
}

/// 超上限才裁：先 count（走索引，毫秒级），避免每次写入都跑一条带 OFFSET 的删除。
/// 裁剪本身不写日志，否则「写日志 → 裁剪 → 又写日志」会自我循环。
/// 顺序只按 id：id 形如 LOG-{12 位十六进制毫秒}-{序号}，固定宽度使字典序 == 时间序，
/// 比秒级粒度的 created_at 更可靠（同秒不误裁新的）。
fn prune_audit_log(conn: &Connection) -> Result<()> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM sys_audit_log", [], |r| r.get(0))?;
    if n > AUDIT_KEEP_MAX {
        conn.execute(
            "DELETE FROM sys_audit_log WHERE id IN ( \
                 SELECT id FROM sys_audit_log \
                 ORDER BY id DESC LIMIT -1 OFFSET ?1)",
            rusqlite::params![AUDIT_KEEP_MAX],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R-68（C5）：账目库必须是 FULL。WAL + NORMAL 只保证不损坏、不保证
    /// 掉电时最近已提交事务还在；这里钉住级别，防止日后被「优化」回 NORMAL。
    #[test]
    fn 持久化级别与删除清零() {
        let conn = open_in_memory().unwrap();
        let sync: i64 = conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sync, 2, "FULL=2；NORMAL=1 会在掉电时丢已提交事务");
        let secure: i64 = conn
            .query_row("PRAGMA secure_delete", [], |r| r.get(0))
            .unwrap();
        assert_eq!(secure, 1, "附件删掉后旧字节不能留在库文件里");
    }

    /// 建库幂等：重复执行不覆盖 sys_meta、不丢日志、不重建视图打断读取。
    #[test]
    fn 建库幂等且不覆盖已有数据() {
        let mut conn = open_in_memory().unwrap();
        init_schema(&mut conn).unwrap();
        set_meta(&conn, "keep_me", "yes", "t").unwrap();
        insert_audit_log(
            &conn,
            "ct_contract",
            "CT-1",
            Some("甲合同"),
            "CREATE",
            Some("建库后写入"),
            "t",
        )
        .unwrap();
        init_schema(&mut conn).unwrap();
        assert_eq!(get_meta(&conn, "keep_me").unwrap().as_deref(), Some("yes"));
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM sys_audit_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        for (name, _) in schema::view_statements() {
            conn.execute_batch(&format!("SELECT * FROM {name} LIMIT 1"))
                .unwrap_or_else(|e| panic!("视图 {name} 不可用：{e}"));
        }
    }

    /// 去掉迁移后库内不再有版本标记，界面直接显示编译期常量；容差恒为 0。
    #[test]
    fn 建库不写版本标记与容差键() {
        let mut conn = open_in_memory().unwrap();
        init_schema(&mut conn).unwrap();
        assert!(get_meta(&conn, "schema_version").unwrap().is_none());
        assert!(get_meta(&conn, "tol_bp").unwrap().is_none());
        assert_eq!(tolerance(&conn).unwrap().bp(), 0);
        // 即使有人手工往 sys_meta 塞了非 0 容差，也不参与额度计算
        set_meta(&conn, "tol_bp", "100", "t").unwrap();
        assert_eq!(tolerance(&conn).unwrap().bp(), 0);
    }

    /// 建一个独立的文件库，返回 (连接, 库路径)。
    /// 用进程 id 隔离，避免并行测试互相删文件。
    fn temp_db(tag: &str) -> (Connection, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("ctp-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.db");
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        let conn = open(path.to_str().unwrap()).unwrap();
        (conn, path)
    }

    fn drop_db(conn: Connection, path: &std::path::Path) {
        drop(conn);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// R-67：附件换成小文件后，库文件必须真的缩小。
    #[test]
    fn 压缩把空闲页还给文件系统() {
        let (conn, path) = temp_db("compact");
        conn.execute_batch("CREATE TABLE t(id TEXT PRIMARY KEY, b BLOB)")
            .unwrap();
        let big = vec![b'X'; 3 * 1024 * 1024];
        conn.execute("INSERT INTO t VALUES('a', ?1)", rusqlite::params![&big])
            .unwrap();
        compact(&conn).unwrap();
        let before = std::fs::metadata(&path).unwrap().len();

        let small = vec![b'Y'; 64 * 1024];
        conn.execute(
            "UPDATE t SET b = ?1 WHERE id = 'a'",
            rusqlite::params![&small],
        )
        .unwrap();
        compact(&conn).unwrap();
        let after = std::fs::metadata(&path).unwrap().len();

        assert!(
            before > 2 * 1024 * 1024,
            "前置：写入 3 MB 后文件应该够大，实际 {before}"
        );
        assert!(
            after * 4 < before,
            "压缩后应显著变小，实际 {before} → {after}"
        );
        let len: i64 = conn
            .query_row("SELECT length(b) FROM t WHERE id = 'a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(len, small.len() as i64);
        drop_db(conn, &path);
    }

    /// R-67：secure_delete 开启后，覆盖写不会把旧内容以明文留在库文件里。
    #[test]
    fn 覆盖写不把旧字节留在文件里() {
        let (conn, path) = temp_db("secure");
        conn.execute_batch("CREATE TABLE t(id TEXT PRIMARY KEY, b BLOB)")
            .unwrap();
        let mut old = b"OLDMARK-UNIQUE-PAYLOAD".to_vec();
        old.extend(std::iter::repeat_n(b'Q', 200_000));
        conn.execute("INSERT INTO t VALUES('a', ?1)", rusqlite::params![&old])
            .unwrap();
        let new = vec![b'N'; 1_000];
        conn.execute(
            "UPDATE t SET b = ?1 WHERE id = 'a'",
            rusqlite::params![&new],
        )
        .unwrap();
        let _: (i64, i64, i64) = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert!(
            !contains(&raw, b"OLDMARK-UNIQUE-PAYLOAD"),
            "被覆盖的旧内容仍能从库文件里搜到，说明 secure_delete 没生效"
        );
        drop_db(conn, &path);
    }
}
