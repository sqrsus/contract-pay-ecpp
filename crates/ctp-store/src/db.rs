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
    // 枚举收口要赶在建视图与首次读取之前：旧码一旦被 `from_code` 拒绝，
    // 未迁移的行会让整个台账读取失败。
    migrate_retired_enums(conn)?;
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

/// R-57：合同类型与支付方式的枚举收口。
///
/// 枚举删除后 `from_code` 不再接受旧码，若库里仍留着旧值，`repo` 读取会直接报错、
/// 整个台账打不开。因此这里做一次性改写（幂等）：
///
/// * `SUPERVISE / DESIGN / SURVEY` → `CONSTRUCT`（界面显示为「工程」）；
/// * `ACCEPTANCE`（承兑）→ `TRANSFER`（转账）。
///
/// 存量库的 CHECK 约束是旧集合，对旧值只是「允许」而非「要求」，改写不需要重建表；
/// 新库 DDL 已是新集合，两句 UPDATE 影响 0 行。
fn migrate_retired_enums(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE ct_contract SET contract_type = 'CONSTRUCT' \
         WHERE contract_type IN ('SUPERVISE','DESIGN','SURVEY')",
        [],
    )?;
    tx.execute(
        "UPDATE pay_payment SET pay_method = 'TRANSFER' WHERE pay_method = 'ACCEPTANCE'",
        [],
    )?;
    tx.commit()?;
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

/// 超付容差（BR-15 的 `tol_bp`）：**业务口径固定为 0**。
///
/// R-33 起界面不再提供任何配置入口，`sys_meta.tol_bp` 仅为兼容旧库保留，
/// 读取一律返回 0，避免旧库里曾保存过的非 0 值继续抬高拦截上限。
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

#[cfg(test)]
mod tests {
    use super::*;

    fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
        let sql = format!("PRAGMA table_info({table})");
        conn.prepare(&sql)
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .any(|c| c == column)
    }

    /// R-39 代表性旧库 fixture：先按当前结构建库并放入业务数据，
    /// 再退回「补列之前」的形态（缺 `target_label` 与 6 个合同扩展列、视图全缺），
    /// 然后验证 migrate 补齐结构、保留数据，且重复运行完全幂等。
    #[test]
    fn 旧库迁移补齐结构且保留数据_重复运行幂等() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        // 代表性存量数据：一条 sys_meta + 一条操作日志
        set_meta(&conn, "legacy_marker", "keep-me", "t").unwrap();
        insert_audit_log(
            &conn,
            "ct_contract",
            "HT-OLD-1",
            Some("旧合同"),
            "CREATE",
            Some("迁移前写入"),
            "t",
        )
        .unwrap();
        let logs_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM sys_audit_log", [], |r| r.get(0))
            .unwrap();

        // 退回旧形态：先摘视图（视图引用待删列），再删列
        for (name, _) in schema::view_statements() {
            conn.execute_batch(&format!("DROP VIEW IF EXISTS {name}"))
                .unwrap();
        }
        conn.execute_batch("ALTER TABLE sys_audit_log DROP COLUMN target_label")
            .unwrap();
        for col in [
            "bid_method",
            "open_bid_date",
            "project_leader",
            "contact_info",
            "bid_agent_id",
            "delivery_date",
        ] {
            conn.execute_batch(&format!("ALTER TABLE ct_contract DROP COLUMN {col}"))
                .unwrap();
        }
        assert!(!has_column(&conn, "sys_audit_log", "target_label"));
        assert!(!has_column(&conn, "ct_contract", "delivery_date"));

        // 连续迁移两次：第一次补齐，第二次必须什么都不错、不重复插数据
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();

        assert!(has_column(&conn, "sys_audit_log", "target_label"));
        for col in [
            "bid_method",
            "open_bid_date",
            "project_leader",
            "contact_info",
            "bid_agent_id",
            "delivery_date",
        ] {
            assert!(has_column(&conn, "ct_contract", col), "{col} 应被补齐");
        }
        assert_eq!(
            get_meta(&conn, "legacy_marker").unwrap().as_deref(),
            Some("keep-me"),
            "存量 sys_meta 不能被覆盖"
        );
        assert_eq!(
            schema_version(&conn).unwrap().as_deref(),
            Some(schema::SCHEMA_VERSION)
        );
        let logs_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM sys_audit_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(logs_after, logs_before, "存量日志不能丢也不能多");
        let old_label: Option<String> = conn
            .query_row(
                "SELECT target_label FROM sys_audit_log WHERE target_id = 'HT-OLD-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(old_label.is_none(), "旧日志新增列应为 NULL，由界面回退显示");

        // 全部视图可查询（旧库重建视图是最容易出错的一步）
        for (name, _) in schema::view_statements() {
            conn.execute_batch(&format!("SELECT * FROM {name} LIMIT 1"))
                .unwrap_or_else(|e| panic!("视图 {name} 不可用：{e}"));
        }
    }

    /// R-57：新库的 CHECK 不再接受退场的枚举值。
    ///
    /// 存量库不会重建表（旧 CHECK 是「超集」，只是放行旧值），因此这里只校验新库；
    /// 存量库的旧值由 `migrate_retired_enums` 一次性改写后，读取路径不会再遇到它们。
    #[test]
    fn 新库拒绝已退场的合同类型与支付方式() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        let contract_ddl: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'ct_contract'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        for retired in ["'SUPERVISE'", "'DESIGN'", "'SURVEY'"] {
            assert!(
                !contract_ddl.contains(retired),
                "合同类型 CHECK 仍含 {retired}"
            );
        }
        let payment_ddl: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'pay_payment'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !payment_ddl.contains("'ACCEPTANCE'"),
            "支付方式 CHECK 仍含已退场的承兑"
        );

        // 迁移函数对全新库是幂等的空操作，连续跑两次不应报错
        migrate_retired_enums(&mut conn).unwrap();
        migrate_retired_enums(&mut conn).unwrap();
    }

    /// 建一个独立的文件库，返回 (连接, 库路径, 临时目录)。
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
    ///
    /// SQLite 的 UPDATE 只把旧 BLOB 占的页挂到 freelist 上复用，文件高水位不回落
    /// （10 MB 换成 1 MB，库仍是 10 MB）；`compact` 的 VACUUM + WAL checkpoint 才会缩。
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
        // 压缩不能改变数据本身
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
        // 把 WAL 合并回主库，否则读到的还是旧主库
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
