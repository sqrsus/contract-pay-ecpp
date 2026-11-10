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

/// 建表 + 补列 + 建索引 + 建视图；重复执行幂等。
///
/// R-68 起带版本守卫（#15 / #22）：
/// * 库内版本**新于**程序 → 报错拒动结构（旧程序开新库，防止降级标记 / 误删新结构）；
/// * 版本相同 → 直接返回，不重放建表 / DROP+CREATE 视图（多实例共享库时不再互相打断）；
/// * 版本较旧或首次打开（无标记）→ 执行完整迁移并回写版本号。
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let existing = schema_version(conn)?;
    if let Some(v) = existing.as_deref() {
        match version_cmp(v, schema::SCHEMA_VERSION) {
            std::cmp::Ordering::Greater => {
                return Err(StoreError::Inconsistent(format!(
                    "数据库结构版本 {v} 高于本程序支持的 {}，请先升级程序后再打开",
                    schema::SCHEMA_VERSION
                )));
            }
            std::cmp::Ordering::Equal => return Ok(()),
            std::cmp::Ordering::Less => {}
        }
    }
    // 角色枚举扩展要在建表之前做：老库的 CHECK 不含 OWNER，只能重建表。
    migrate_contractor_role(conn)?;
    // C10：先处理同名单位再建唯一索引，否则索引创建直接失败、程序被卡在启动
    dedupe_contractor_names(conn)?;
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
    // C13-B：退场列（print_count）删除；C8：可归属的变更历史搬进历史表
    migrate_drop_retired_columns(conn)?;
    migrate_change_history(conn)?;
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
    // C7-B：序号表按库内已有编号校准（只升不降），防撞号死循环
    calibrate_sequences(conn)?;
    set_meta(conn, "schema_version", schema::SCHEMA_VERSION, "system")?;
    if get_meta(conn, "tol_bp")?.is_none() {
        set_meta(conn, "tol_bp", "0", "system")?;
    }
    Ok(())
}

/// 库内结构版本；`sys_meta` 还没建（首次打开）时返回 `None`。
pub fn schema_version(conn: &Connection) -> Result<Option<String>> {
    let has_table: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'sys_meta'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if has_table.is_none() {
        return Ok(None);
    }
    get_meta(conn, "schema_version")
}

/// 结构版本比较："1.6.0-rv26" 拆成（数字三元组, rv 序号）逐段比较。
/// 字符串直比在 rv 两位数时会错序（"rv9" > "rv26"），必须数值化。
fn version_rank(v: &str) -> (Vec<i64>, i64) {
    let (core, rv) = match v.split_once("-rv") {
        Some((c, r)) => (c, r.parse::<i64>().unwrap_or(0)),
        None => (v, 0),
    };
    let nums: Vec<i64> = core.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    (nums, rv)
}

fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    version_rank(a).cmp(&version_rank(b))
}

/// C10：同名（未删除）单位在加唯一索引前一次性改名「X(2)」，每条留审计日志。
///
/// 保留 rowid 最小（最早建档）的那一个不动；后续的按出现顺序追加序号。
/// 现库无重名时整段是空操作；新库（表还没建）直接返回。
fn dedupe_contractor_names(conn: &mut Connection) -> Result<()> {
    let has_table: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'org_contractor'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if has_table.is_none() {
        return Ok(());
    }
    let rows: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, contractor_name FROM org_contractor \
             WHERE is_deleted = 0 AND contractor_name IN ( \
                 SELECT contractor_name FROM org_contractor WHERE is_deleted = 0 \
                 GROUP BY contractor_name HAVING COUNT(*) > 1) \
             ORDER BY contractor_name, rowid",
        )?;
        let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        it.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if rows.is_empty() {
        return Ok(());
    }
    let now = now_iso();
    let tx = conn.transaction()?;
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (id, name) in rows {
        let n = seen.entry(name.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            continue; // 同名里最早建档的那个保留原名
        }
        let new_name = format!("{name}({n})");
        tx.execute(
            "UPDATE org_contractor SET contractor_name = ?2, updated_at = ?3, \
                updated_by = 'system', version = version + 1 WHERE id = ?1",
            rusqlite::params![id, new_name, now],
        )?;
        insert_audit_log(
            &tx,
            "org_contractor",
            &id,
            Some(&new_name),
            "RENAME",
            Some(&format!(
                "同名单位一次性改名：{name} → {new_name}（单位名唯一索引启用前整理）"
            )),
            "system",
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// C13-B：删除已退场的 `pay_payment.print_count` 列（全库无写入点、恒为 0）。
fn migrate_drop_retired_columns(conn: &mut Connection) -> Result<()> {
    let existing: Vec<String> = conn
        .prepare("PRAGMA table_info(pay_payment)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if existing.iter().any(|c| c == "print_count") {
        // 无索引 / 视图引用该列，可直接 DROP；列带 DEFAULT 0，保留也不影响写入，
        // 但验收口径里「恒为 0 的打印次数」会被当成缺陷，删干净。
        conn.execute_batch("ALTER TABLE pay_payment DROP COLUMN print_count")?;
    }
    Ok(())
}

/// C8：把操作日志里**可归属到合同**的金额 / 审计价变更搬进历史表。
///
/// 以日志 id 作历史表主键，天然幂等（重复执行 INSERT OR IGNORE 全部跳过）。
/// 金额的前后值不回填（日志 detail 是拼接文本，解析不可靠），`old_cent` /
/// `new_cent` 留空，`reason` 存 detail 原文；新变更由 service 层直接写全字段。
fn migrate_change_history(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO ct_contract_change_history \
             (id, contract_id, change_type, old_cent, new_cent, reason, operator_id, created_at) \
         SELECT l.id, l.target_id, \
                CASE l.action WHEN 'EDIT_AMOUNT' THEN 'AMOUNT' ELSE 'AUDIT_PRICE' END, \
                NULL, NULL, l.detail, l.operator_id, l.created_at \
         FROM sys_audit_log l \
         WHERE l.target_type = 'ct_contract' \
           AND l.action IN ('EDIT_AMOUNT','SET_AUDIT_PRICE') \
           AND EXISTS (SELECT 1 FROM ct_contract c WHERE c.id = l.target_id)",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

/// C7-B：序号表按库内已有编号校准，**只升不降**。
///
/// 取号（`next_sequence`）在业务事务里自增，事务因任何原因回滚时序号不前进；
/// 若库内已存在更大编号（备份恢复、历史手工插入），后续取号会一直撞号且
/// 唯一索引令每次提交失败。这里按三个编号族的年度最大后缀抬高 `next_val`。
fn calibrate_sequences(conn: &mut Connection) -> Result<()> {
    let now = now_iso();
    let tx = conn.transaction()?;
    // (编号列, 表, 编号前缀, 序号 scope 前缀)；年度 / 后缀起点由前缀推导
    for (col, table, prefix, scope) in [
        ("contract_no", "ct_contract", "HT", "contract_no"),
        ("payment_no", "pay_payment", "BK", "payment_no"),
        ("project_code", "prj_project", "PRJ", "project_code"),
    ] {
        let sql = format!(
            "SELECT substr({col}, {y_at}, 4) AS y, \
                    MAX(CAST(substr({col}, {s_at}) AS INTEGER)) AS n \
             FROM {table} \
             WHERE {col} GLOB '{prefix}-[0-9][0-9][0-9][0-9]-[0-9]*' \
             GROUP BY y",
            col = col,
            table = table,
            prefix = prefix,
            y_at = match prefix {
                "PRJ" => 5, // P R J - y e a r - s e q → 年度从第 5 字符起
                _ => 4,     // H T - y e a r - s e q → 年度从第 4 字符起
            },
            s_at = match prefix {
                "PRJ" => 10,
                _ => 9,
            },
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (year, max_seq) in rows {
            if max_seq <= 0 {
                continue;
            }
            // next_val 取 max+1（下一个要发的号）；只在比现值大时才抬
            tx.execute(
                "INSERT INTO sys_sequence(scope_key, next_val, created_at, updated_at, version) \
                 VALUES (?1, ?2, ?3, ?3, 1) \
                 ON CONFLICT(scope_key) DO UPDATE SET \
                    next_val = excluded.next_val, updated_at = excluded.updated_at, \
                    version = sys_sequence.version + 1 \
                 WHERE excluded.next_val > sys_sequence.next_val",
                rusqlite::params![format!("{scope}:{year}"), max_seq + 1, now],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
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
    // R-68（#20）：裁剪顺序只按 id——id 形如 LOG-{12 位十六进制毫秒}-{序号}，
    // 固定宽度使字典序 == 时间序，比秒级粒度的 created_at 更可靠（同秒不误裁新的）。
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

    fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
        let sql = format!("PRAGMA table_info({table})");
        conn.prepare(&sql)
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .any(|c| c == column)
    }

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

    /// #15：rv 序号按数值比较，字典序会把 rv9 误判成比 rv27 新而拒绝启动。
    #[test]
    fn 结构版本按数字段比较() {
        use std::cmp::Ordering;
        assert_eq!(version_cmp("1.6.0-rv9", "1.6.0-rv27"), Ordering::Less);
        assert_eq!(version_cmp("1.6.0-rv27", "1.6.0-rv27"), Ordering::Equal);
        assert_eq!(version_cmp("1.6.0-rv28", "1.6.0-rv27"), Ordering::Greater);
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

        // 连续迁移两次：第一次补齐，第二次必须什么都不错、不重复插数据。
        // R-68 版本守卫：先把版本标记退回旧值，模拟“真正的旧库”
        // （仅删列不退版本的话，版本相同会被直接跳过）。
        set_meta(&conn, "schema_version", "1.6.0-rv1", "t").unwrap();
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

    /// R-68（#15）：库版本高于程序时，迁移必须拒绝执行而不是回写低版本号。
    #[test]
    fn 库版本高于程序时拒绝迁移() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        set_meta(&conn, "schema_version", "1.6.0-rv9999", "t").unwrap();
        let err = migrate(&mut conn).unwrap_err();
        assert!(err.to_string().contains("高于本程序支持"), "实际：{err}");
        // 版本标记不能被旧程序改写
        assert_eq!(
            get_meta(&conn, "schema_version").unwrap().as_deref(),
            Some("1.6.0-rv9999")
        );
    }

    /// R-68（#22）：版本相同时迁移直接跳过——视图被外部改坏也不重建，
    /// 多实例共享库时不会互相打断。
    #[test]
    fn 版本相同则跳过迁移() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        // 人为摘掉一个视图；版本相同 → migrate 不重建
        conn.execute_batch("DROP VIEW v_payment_period").unwrap();
        migrate(&mut conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='view' AND name='v_payment_period'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "版本相同不应重建视图");
    }

    /// R-68（C13-B）：旧库的 print_count 列被删除，新库根本不建。
    #[test]
    fn print_count_列被移除() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        assert!(
            !has_column(&conn, "pay_payment", "print_count"),
            "新库不应有该列"
        );
        // 模拟旧库：补回列再降版本，迁移应把它删掉
        set_meta(&conn, "schema_version", "1.6.0-rv1", "t").unwrap();
        conn.execute_batch(
            "ALTER TABLE pay_payment ADD COLUMN print_count INTEGER NOT NULL DEFAULT 0",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        assert!(
            !has_column(&conn, "pay_payment", "print_count"),
            "旧库列应被 DROP"
        );
    }

    /// R-68（C10）：同名单位在加唯一索引前自动改名，最早的保留原名，改名留审计。
    #[test]
    fn 同名单位迁移时自动改名() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        set_meta(&conn, "schema_version", "1.6.0-rv1", "t").unwrap();
        // 模拟旧库：唯一索引还不存在，才可能已经存着同名单位
        conn.execute_batch("DROP INDEX IF EXISTS ux_contractor_name")
            .unwrap();
        let now = now_iso();
        for (i, name) in ["甲公司", "甲公司", "甲公司", "乙公司"].iter().enumerate() {
            conn.execute(
                "INSERT INTO org_contractor(id, contractor_name, role_type, created_at, updated_at, version) \
                 VALUES (?1, ?2, 'CONSTRUCT', ?3, ?3, 1)",
                rusqlite::params![format!("ORG-{i}"), name, now],
            )
            .unwrap();
        }
        migrate(&mut conn).unwrap();
        let names: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT contractor_name FROM org_contractor ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(names, vec!["甲公司", "甲公司(2)", "甲公司(3)", "乙公司"]);
        // 改名动作留了审计
        let renamed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sys_audit_log WHERE action = 'RENAME'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(renamed, 2);
        // 唯一索引就位：再插同名必须失败
        conn.execute(
            "INSERT INTO org_contractor(id, contractor_name, role_type, created_at, updated_at, version) \
             VALUES ('ORG-X', '甲公司', 'CONSTRUCT', ?1, ?1, 1)",
            rusqlite::params![now],
        )
        .unwrap_err();
    }

    /// R-68（C7-B）：序号表按库内已有编号校准——手工插入 HT-2026-009 后，
    /// 下一次取号必须是 10，而不是撞在 9 上反复失败。
    #[test]
    fn 序号表按库内最大编号校准() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        set_meta(&conn, "schema_version", "1.6.0-rv1", "t").unwrap();
        let now = now_iso();
        conn.execute(
            "INSERT INTO prj_project(id, project_code, project_name, created_at, updated_at, version) \
             VALUES ('PRJ-1', 'PRJ-2026-001', '甲项目', ?1, ?1, 1)",
            rusqlite::params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO org_contractor(id, contractor_name, role_type, created_at, updated_at, version) \
             VALUES ('ORG-1', '甲公司', 'CONSTRUCT', ?1, ?1, 1)",
            rusqlite::params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ct_contract(id, project_id, contract_no, contract_name, contract_type, \
                contract_category_code, party_a, contractor_id, sign_date, original_amount_cent, \
                status, payment_terms, created_at, updated_at, version) \
             VALUES ('CT-1', 'PRJ-1', 'HT-2026-009', '手工补录', 'CONSTRUCT', 'GC', '甲方', 'ORG-1', \
                '2026-01-01', 100, 'DRAFT', '', ?1, ?1, 1)",
            rusqlite::params![now],
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let seq = next_sequence(&conn, "contract_no:2026").unwrap();
        assert!(seq >= 10, "校准后取号应 ≥ 10，实际 {seq}");
    }

    /// R-68（C8）：操作日志里的金额 / 审计价变更被搬进历史表，且迁移幂等。
    #[test]
    fn 变更历史从操作日志迁移且幂等() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        let now = now_iso();
        conn.execute(
            "INSERT INTO prj_project(id, project_code, project_name, created_at, updated_at, version) \
             VALUES ('PRJ-1', 'PRJ-2026-001', '甲项目', ?1, ?1, 1)",
            rusqlite::params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO org_contractor(id, contractor_name, role_type, created_at, updated_at, version) \
             VALUES ('ORG-1', '甲公司', 'CONSTRUCT', ?1, ?1, 1)",
            rusqlite::params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ct_contract(id, project_id, contract_no, contract_name, contract_type, \
                contract_category_code, party_a, contractor_id, sign_date, original_amount_cent, \
                status, payment_terms, created_at, updated_at, version) \
             VALUES ('CT-1', 'PRJ-1', 'HT-2026-001', '甲合同', 'CONSTRUCT', 'GC', '甲方', 'ORG-1', \
                '2026-01-01', 100, 'DRAFT', '', ?1, ?1, 1)",
            rusqlite::params![now],
        )
        .unwrap();
        insert_audit_log(
            &conn,
            "ct_contract",
            "CT-1",
            Some("HT-2026-001 甲合同"),
            "EDIT_AMOUNT",
            Some("100.00 -> 200.00：补充协议"),
            "tester",
        )
        .unwrap();
        set_meta(&conn, "schema_version", "1.6.0-rv1", "t").unwrap();
        migrate(&mut conn).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM ct_contract_change_history", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 1, "日志里可归属的变更应搬进历史表");
        // 幂等：再来一次不多插
        set_meta(&conn, "schema_version", "1.6.0-rv1", "t").unwrap();
        migrate(&mut conn).unwrap();
        let n2: i64 = conn
            .query_row("SELECT COUNT(*) FROM ct_contract_change_history", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n2, 1);
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
