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
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

/// 建表 + 建索引 + 建视图，全部在一个事务内完成；重复执行幂等。
pub fn migrate(conn: &mut Connection) -> Result<()> {
    {
        let tx = conn.transaction()?;
        for (_, sql) in schema::table_statements() {
            tx.execute_batch(sql)?;
        }
        for sql in schema::index_statements() {
            tx.execute_batch(sql)?;
        }
        for (_, sql) in schema::view_statements() {
            tx.execute_batch(&sql)?;
        }
        tx.commit()?;
    }
    set_meta(conn, "schema_version", schema::SCHEMA_VERSION, "system")?;
    if get_meta(conn, "tol_bp")?.is_none() {
        set_meta(conn, "tol_bp", "0", "system")?;
    }
    Ok(())
}

pub fn schema_version(conn: &Connection) -> Result<Option<String>> {
    get_meta(conn, "schema_version")
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

/// 容差率（BR-15 的 `tol_bp`），缺省 0。
pub fn tolerance(conn: &Connection) -> Result<Rate> {
    let bp = get_meta(conn, "tol_bp")?
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(0);
    Ok(Rate::from_bp(bp))
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

pub fn insert_audit_log(
    conn: &Connection,
    target_type: &str,
    target_id: &str,
    action: &str,
    detail: Option<&str>,
    operator_id: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sys_audit_log(id, target_type, target_id, action, detail, operator_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            ctp_core::ids::new_id("LOG"),
            target_type,
            target_id,
            action,
            detail,
            operator_id,
            now_iso()
        ],
    )?;
    Ok(())
}
