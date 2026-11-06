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

/// 建表 + 补列 + 建索引 + 建视图；重复执行幂等。
pub fn migrate(conn: &mut Connection) -> Result<()> {
    // 角色枚举扩展要在建表之前做：老库的 CHECK 不含 OWNER，只能重建表。
    migrate_contractor_role(conn)?;
    {
        let tx = conn.transaction()?;
        for (_, sql) in schema::table_statements() {
            tx.execute_batch(sql)?;
        }
        for sql in schema::index_statements() {
            tx.execute_batch(sql)?;
        }
        tx.commit()?;
    }
    // 先补列、再建视图：视图会引用新增列，顺序反了存量库会报 no such column。
    migrate_contract_columns(conn)?;
    migrate_audit_log_columns(conn)?;
    {
        let tx = conn.transaction()?;
        // `CREATE VIEW IF NOT EXISTS` 不会覆盖已有视图定义；视图列变化时
        // 必须先按依赖倒序删除再重建，否则存量库仍读到旧列。
        let views = schema::view_statements();
        for (name, _) in views.iter().rev() {
            tx.execute_batch(&format!("DROP VIEW IF EXISTS {name}"))?;
        }
        for (_, sql) in &views {
            tx.execute_batch(sql)?;
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

/// 给存量 `ct_contract` 追加新建合同页扩展字段（全部选填，缺列才加，幂等）。
///
/// SQLite `ALTER TABLE … ADD COLUMN` 不支持 `IF NOT EXISTS`，故先用
/// `PRAGMA table_info` 探明已有列，只对缺失的列追加；新库在建表阶段已含这些列，此处跳过。
/// 这些列不参与任何视图计算，追加不会触发视图失效，无需重建视图。
fn migrate_contract_columns(conn: &mut Connection) -> Result<()> {
    let existing: Vec<String> = conn
        .prepare("PRAGMA table_info(ct_contract)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has = |name: &str| existing.iter().any(|c| c == name);
    let tx = conn.transaction()?;
    // 顺序需与 schema.rs 的 ct_contract DDL 保持一致
    let columns: &[(&str, &str)] = &[
        (
            "bid_method",
            "ALTER TABLE ct_contract ADD COLUMN bid_method TEXT \
             CHECK (bid_method IS NULL OR bid_method IN ('OPEN','GOV_PROC','DIRECT'))",
        ),
        (
            "open_bid_date",
            "ALTER TABLE ct_contract ADD COLUMN open_bid_date TEXT",
        ),
        (
            "project_leader",
            "ALTER TABLE ct_contract ADD COLUMN project_leader TEXT",
        ),
        (
            "contact_info",
            "ALTER TABLE ct_contract ADD COLUMN contact_info TEXT",
        ),
        (
            "bid_agent_id",
            "ALTER TABLE ct_contract ADD COLUMN bid_agent_id TEXT REFERENCES org_contractor(id)",
        ),
        (
            "delivery_date",
            "ALTER TABLE ct_contract ADD COLUMN delivery_date TEXT",
        ),
    ];
    for (name, sql) in columns {
        if !has(name) {
            tx.execute_batch(sql)?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// 给存量 `sys_audit_log` 追加 `target_label`（R-31：操作对象的可读名称）。
///
/// 与 `migrate_contract_columns` 同法：`ALTER TABLE … ADD COLUMN` 没有
/// `IF NOT EXISTS`，先探列再补。旧日志的该列为 NULL，页面上回退显示目标 id。
fn migrate_audit_log_columns(conn: &mut Connection) -> Result<()> {
    let existing: Vec<String> = conn
        .prepare("PRAGMA table_info(sys_audit_log)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if existing.iter().any(|c| c == "target_label") {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE sys_audit_log ADD COLUMN target_label TEXT")?;
    Ok(())
}

/// 把 `org_contractor.role_type` 的 CHECK 扩到含 `OWNER` 与 `AGENT`。
///
/// SQLite 无法修改已有 CHECK 约束，只能「建新表 → 搬数据 → 换名」。
/// 因为 `ct_contract.contractor_id` 引用本表，重建时必须临时关闭外键
/// （`PRAGMA foreign_keys` 在事务内不生效，故放在事务外切换）。
/// 已是新结构（新库或已迁移）时直接返回，保证幂等。
fn migrate_contractor_role(conn: &mut Connection) -> Result<()> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'org_contractor'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(ddl) = existing else {
        // 新库：表还不存在，后面建表语句直接就是新结构
        return Ok(());
    };
    // 已是含 OWNER 与 AGENT 的新结构，直接返回
    if ddl.contains("'OWNER'") && ddl.contains("'AGENT'") {
        return Ok(());
    }

    // 复用同一份建表语句，只把表名换掉，避免两处 DDL 漂移
    let create = schema::table_statements()
        .into_iter()
        .find(|(name, _)| *name == "org_contractor")
        .map(|(_, sql)| {
            sql.replace(
                "CREATE TABLE IF NOT EXISTS org_contractor",
                "CREATE TABLE org_contractor_migrated",
            )
        })
        .unwrap_or_default();
    if create.trim().is_empty() {
        return Ok(());
    }

    conn.pragma_update(None, "foreign_keys", "OFF")?;
    let outcome = (|| -> Result<()> {
        let views = schema::view_statements();
        let tx = conn.transaction()?;
        // 视图引用 org_contractor：不先摘掉，DROP TABLE 会让 SQLite 判定 schema 损坏
        for (name, _) in &views {
            tx.execute_batch(&format!("DROP VIEW IF EXISTS {name}"))?;
        }
        tx.execute_batch(&create)?;
        tx.execute_batch(
            "INSERT INTO org_contractor_migrated \
                (id, contractor_name, role_type, credit_code, bank_name, bank_account, \
                 remark, is_deleted, created_at, created_by, updated_at, updated_by, version) \
             SELECT id, contractor_name, role_type, credit_code, bank_name, bank_account, \
                 remark, is_deleted, created_at, created_by, updated_at, updated_by, version \
             FROM org_contractor;
             DROP TABLE org_contractor;
             ALTER TABLE org_contractor_migrated RENAME TO org_contractor;",
        )?;
        for (_, sql) in &views {
            tx.execute_batch(sql)?;
        }
        tx.commit()?;
        Ok(())
    })();
    conn.pragma_update(None, "foreign_keys", "ON")?;
    outcome
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

/// 日志保留上限（R-31）：超过就静默裁掉最旧的若干条，防止库无限膨胀。
///
/// 5 万条按每天 50 次操作算约三年，日常使用不会触发；触发时用户仍可在
/// 「操作日志」页看到最近的全部记录。
pub const AUDIT_KEEP_MAX: i64 = 50_000;

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
    // 超上限才裁：先 count（走索引，毫秒级），避免每次写入都跑一条带 OFFSET 的删除。
    // 裁剪本身不写日志，否则「写日志 → 裁剪 → 又写日志」会自我循环。
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM sys_audit_log", [], |r| r.get(0))?;
    if n > AUDIT_KEEP_MAX {
        conn.execute(
            "DELETE FROM sys_audit_log WHERE id IN ( \
                 SELECT id FROM sys_audit_log \
                 ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?1)",
            rusqlite::params![AUDIT_KEEP_MAX],
        )?;
    }
    Ok(())
}
