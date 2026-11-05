//! 建表语句与视图定义（文档 5.4 / 5.5 的落地版）。
//!
//! 相对文档原稿的改动全部来自评审意见，逐条可追溯：
//!
//! | 评审 | 落地 |
//! |---|---|
//! | H1 | `rate_bp_expr` 把 `*2` 放回分母，与 `ctp_core::Rate::ratio` 逐位一致；`selfcheck` 断言差异为 0 |
//! | H2 | 已付净额只经 `paid_net_subquery`，直连 `pay_payment`；仓储与视图共用同一段 SQL 与状态集合 |
//! | H3 | 不建 `pay_application` / `pay_application_item` / `pay_approval_log` / `pay_payment_allocation` |
//! | H4 | 合同类别落 `sys_dict_item`，会计分类落 `ct_category`，两套字典编码不重叠 |
//! | H5 | `pay_payment.status ∈ ('PAID','VOID')`，无 `reversal_of`；作废替代红冲 |
//! | M3 | 不建 `ix_contract_audited`（5.6 已否决）；无 `application_id` 悬空外键 |
//! | M4 | 业务表补齐 `created_at/created_by/updated_at/updated_by/version`；仅 `sys_audit_log` 豁免（追加型日志） |
//!
//! 本期建表 13 张 + 视图 4 个；文档目标态为 21 张，其余随移出模块或二期补建。

/// 结构版本，写入 `PRAGMA user_version` 的伴生元数据键 `schema_version`。
pub const SCHEMA_VERSION: &str = "1.6.0-rv24";

/// 计入「已付净额」的单据状态（**唯一口径**，视图与仓储共用）。
pub const PAID_STATUS_SQL: &str = "'PAID'";

/// DB-06 豁免清单：追加型日志表不要求 `updated_*` 与 `version`。
pub const AUDIT_COLUMN_EXEMPT: &[&str] = &["sys_audit_log"];

/// 已付净额子查询。`contract_col` 为合同列名（子查询内固定把 `pay_payment` 别名成 `pm`，
/// 故传 `"pm.contract_id"`）。
pub fn paid_net_subquery(contract_col: &str) -> String {
    format!(
        "SELECT {contract_col} AS contract_id, SUM(amount_cent) AS paid_cent \
         FROM pay_payment pm WHERE pm.is_deleted = 0 AND pm.status IN ({PAID_STATUS_SQL}) \
         GROUP BY {contract_col}"
    )
}

/// 支付比率的 SQL 表达式，与 `Rate::ratio` 逐位一致：
/// 展开 `(n*2 ± d)/(d*2)` 得 `(numer*20000 ± denom) / (denom*2)`。
/// **`*2` 必须在分母里**——评审 H1 的原始缺陷就是写成了「除完再乘 2」。
pub fn rate_bp_expr(numer: &str, denom: &str) -> String {
    format!(
        "CASE WHEN {denom} > 0 \
         THEN CASE WHEN {numer} >= 0 \
              THEN ({numer} * 20000 + {denom}) / ({denom} * 2) \
              ELSE ({numer} * 20000 - {denom}) / ({denom} * 2) END \
         ELSE 0 END"
    )
}

/// 有效计价基准表达式（BR-14）。
pub fn base_expr(alias: &str) -> String {
    format!(
        "CASE WHEN {alias}.audit_price_cent > 0 THEN {alias}.audit_price_cent \
         ELSE {alias}.original_amount_cent END"
    )
}

/// 全部建表语句（顺序即执行顺序）。
pub fn table_statements() -> Vec<(&'static str, &'static str)> {
    vec![
        ("sys_dict_item", SYS_DICT_ITEM),
        ("sys_sequence", SYS_SEQUENCE),
        ("sys_meta", SYS_META),
        ("sys_audit_log", SYS_AUDIT_LOG),
        ("prj_project", PRJ_PROJECT),
        ("prj_building", PRJ_BUILDING),
        ("org_contractor", ORG_CONTRACTOR),
        ("org_contact", ORG_CONTACT),
        ("ct_category", CT_CATEGORY),
        ("ct_contract", CT_CONTRACT),
        ("ct_contract_item", CT_CONTRACT_ITEM),
        ("ct_contract_attachment", CT_CONTRACT_ATTACHMENT),
        ("pay_payment", PAY_PAYMENT),
    ]
}

/// 索引语句。
pub fn index_statements() -> Vec<&'static str> {
    vec![
        "CREATE INDEX IF NOT EXISTS ix_dict_type ON sys_dict_item(dict_type, sort_no)",
        "CREATE INDEX IF NOT EXISTS ix_building_project ON prj_building(project_id)",
        "CREATE INDEX IF NOT EXISTS ix_contact_contractor ON org_contact(contractor_id)",
        "CREATE UNIQUE INDEX IF NOT EXISTS ux_category_code ON ct_category(category_code)",
        "CREATE UNIQUE INDEX IF NOT EXISTS ux_contract_no ON ct_contract(contract_no) WHERE is_deleted = 0",
        // 评审 M3：文档 5.6 已否决 ix_contract_audited（选择性低、写入有维护成本），此处不建。
        "CREATE INDEX IF NOT EXISTS ix_contract_project ON ct_contract(project_id, status)",
        "CREATE INDEX IF NOT EXISTS ix_contract_org ON ct_contract(contractor_id)",
        "CREATE INDEX IF NOT EXISTS ix_ct_item_contract ON ct_contract_item(contract_id)",
        "CREATE INDEX IF NOT EXISTS ix_ct_item_category ON ct_contract_item(category_id)",
        // 每类附件只允许一份：唯一索引兜底，界面替换时走 UPDATE 而非新增
        "CREATE UNIQUE INDEX IF NOT EXISTS ux_attachment_kind ON ct_contract_attachment(contract_id, att_type)",
        "CREATE UNIQUE INDEX IF NOT EXISTS ux_payment_no ON pay_payment(payment_no) WHERE is_deleted = 0",
        "CREATE INDEX IF NOT EXISTS ix_payment_date ON pay_payment(pay_date, status)",
        "CREATE INDEX IF NOT EXISTS ix_payment_contract ON pay_payment(contract_id, pay_date)",
        "CREATE INDEX IF NOT EXISTS ix_payment_project ON pay_payment(project_id, pay_date)",
        "CREATE INDEX IF NOT EXISTS ix_audit_time ON sys_audit_log(created_at)",
        "CREATE INDEX IF NOT EXISTS ix_audit_target ON sys_audit_log(target_type, target_id)",
    ]
}

const SYS_DICT_ITEM: &str = r#"
CREATE TABLE IF NOT EXISTS sys_dict_item (
    dict_type   TEXT NOT NULL,
    item_code   TEXT NOT NULL,
    item_name   TEXT NOT NULL,
    sort_no     INTEGER NOT NULL DEFAULT 0,
    is_enabled  INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0,1)),
    created_at  TEXT NOT NULL,
    created_by  TEXT,
    updated_at  TEXT NOT NULL,
    updated_by  TEXT,
    version     INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (dict_type, item_code)
)
"#;

const SYS_SEQUENCE: &str = r#"
CREATE TABLE IF NOT EXISTS sys_sequence (
    scope_key   TEXT PRIMARY KEY,
    next_val    INTEGER NOT NULL DEFAULT 0 CHECK (next_val >= 0),
    created_at  TEXT NOT NULL,
    created_by  TEXT,
    updated_at  TEXT NOT NULL,
    updated_by  TEXT,
    version     INTEGER NOT NULL DEFAULT 1
)
"#;

const SYS_META: &str = r#"
CREATE TABLE IF NOT EXISTS sys_meta (
    meta_key    TEXT PRIMARY KEY,
    meta_value  TEXT,
    created_at  TEXT NOT NULL,
    created_by  TEXT,
    updated_at  TEXT NOT NULL,
    updated_by  TEXT,
    version     INTEGER NOT NULL DEFAULT 1
)
"#;

/// 追加型日志表：DB-06 的显式豁免项（评审 M4）。
const SYS_AUDIT_LOG: &str = r#"
CREATE TABLE IF NOT EXISTS sys_audit_log (
    id           TEXT PRIMARY KEY,
    target_type  TEXT NOT NULL,
    target_id    TEXT NOT NULL,
    action       TEXT NOT NULL,
    detail       TEXT,
    operator_id  TEXT,
    created_at   TEXT NOT NULL
)
"#;

const PRJ_PROJECT: &str = r#"
CREATE TABLE IF NOT EXISTS prj_project (
    id            TEXT PRIMARY KEY,
    project_code  TEXT NOT NULL,
    project_name  TEXT NOT NULL,
    remark        TEXT,
    is_deleted    INTEGER NOT NULL DEFAULT 0 CHECK (is_deleted IN (0,1)),
    created_at    TEXT NOT NULL,
    created_by    TEXT,
    updated_at    TEXT NOT NULL,
    updated_by    TEXT,
    version       INTEGER NOT NULL DEFAULT 1
)
"#;

const PRJ_BUILDING: &str = r#"
CREATE TABLE IF NOT EXISTS prj_building (
    id             TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL REFERENCES prj_project(id),
    building_name  TEXT NOT NULL,
    floor_area_x100 INTEGER NOT NULL DEFAULT 0 CHECK (floor_area_x100 >= 0),
    structure_type TEXT,
    floor_count    INTEGER NOT NULL DEFAULT 0 CHECK (floor_count >= 0),
    is_deleted     INTEGER NOT NULL DEFAULT 0 CHECK (is_deleted IN (0,1)),
    created_at     TEXT NOT NULL,
    created_by     TEXT,
    updated_at     TEXT NOT NULL,
    updated_by     TEXT,
    version        INTEGER NOT NULL DEFAULT 1
)
"#;

const ORG_CONTRACTOR: &str = r#"
CREATE TABLE IF NOT EXISTS org_contractor (
    id              TEXT PRIMARY KEY,
    contractor_name TEXT NOT NULL,
    role_type       TEXT NOT NULL DEFAULT 'CONSTRUCT'
                    CHECK (role_type IN ('CONSTRUCT','SUPERVISE','DESIGN','SURVEY','SUPPLIER','OWNER','OTHER')),
    credit_code     TEXT,
    bank_name       TEXT,
    bank_account    TEXT,
    remark          TEXT,
    is_deleted      INTEGER NOT NULL DEFAULT 0 CHECK (is_deleted IN (0,1)),
    created_at      TEXT NOT NULL,
    created_by      TEXT,
    updated_at      TEXT NOT NULL,
    updated_by      TEXT,
    version         INTEGER NOT NULL DEFAULT 1
)
"#;

const ORG_CONTACT: &str = r#"
CREATE TABLE IF NOT EXISTS org_contact (
    id             TEXT PRIMARY KEY,
    contractor_id  TEXT NOT NULL REFERENCES org_contractor(id),
    contact_name   TEXT NOT NULL,
    phone          TEXT,
    is_primary     INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0,1)),
    created_at     TEXT NOT NULL,
    created_by     TEXT,
    updated_at     TEXT NOT NULL,
    updated_by     TEXT,
    version        INTEGER NOT NULL DEFAULT 1
)
"#;

/// 会计分类字典（财务口径）。**合同类别不在这里**，在 `sys_dict_item:CONTRACT_CATEGORY`（评审 H4）。
const CT_CATEGORY: &str = r#"
CREATE TABLE IF NOT EXISTS ct_category (
    id             TEXT PRIMARY KEY,
    category_code  TEXT NOT NULL,
    category_name  TEXT NOT NULL,
    sort_no        INTEGER NOT NULL DEFAULT 0,
    is_enabled     INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0,1)),
    created_at     TEXT NOT NULL,
    created_by     TEXT,
    updated_at     TEXT NOT NULL,
    updated_by     TEXT,
    version        INTEGER NOT NULL DEFAULT 1
)
"#;

const CT_CONTRACT: &str = r#"
CREATE TABLE IF NOT EXISTS ct_contract (
    id                      TEXT PRIMARY KEY,
    project_id              TEXT NOT NULL REFERENCES prj_project(id),
    contract_no             TEXT NOT NULL,
    contract_name           TEXT NOT NULL,
    contract_type           TEXT NOT NULL
                            CHECK (contract_type IN ('CONSTRUCT','SUPERVISE','DESIGN','SURVEY','SUPPLY','SERVICE')),
    contract_category_code  TEXT NOT NULL
                            CHECK (length(trim(contract_category_code)) > 0),
    category_id             TEXT REFERENCES ct_category(id),
    party_a                 TEXT NOT NULL,
    contractor_id           TEXT NOT NULL REFERENCES org_contractor(id),
    building_id             TEXT REFERENCES prj_building(id),
    sort_no                 INTEGER NOT NULL DEFAULT 0,
    sign_date               TEXT NOT NULL,
    original_amount_cent    INTEGER NOT NULL CHECK (original_amount_cent >= 0),
    audit_price_cent        INTEGER,
    audit_date              TEXT,
    audit_org               TEXT,
    audit_report_no         TEXT,
    duration_days           INTEGER NOT NULL DEFAULT 0 CHECK (duration_days >= 0),
    planned_start           TEXT,
    planned_end             TEXT,
    actual_start            TEXT,
    actual_end              TEXT,
    pricing_method          TEXT CHECK (pricing_method IN ('LUMP_SUM','UNIT_PRICE','COST_PLUS')),
    retention_rate_bp       INTEGER NOT NULL DEFAULT 300 CHECK (retention_rate_bp BETWEEN 0 AND 10000),
    advance_rate_bp         INTEGER NOT NULL DEFAULT 0 CHECK (advance_rate_bp BETWEEN 0 AND 10000),
    status                  TEXT NOT NULL DEFAULT 'DRAFT'
                            CHECK (status IN ('DRAFT','PENDING','ACTIVE','PERFORMING','COMPLETED','SETTLED','CLOSED','VOID')),
    payment_terms           TEXT NOT NULL DEFAULT '',
    remark                  TEXT,
    is_deleted              INTEGER NOT NULL DEFAULT 0 CHECK (is_deleted IN (0,1)),
    created_at              TEXT NOT NULL,
    created_by              TEXT,
    updated_at              TEXT NOT NULL,
    updated_by              TEXT,
    version                 INTEGER NOT NULL DEFAULT 1,
    CHECK (planned_end IS NULL OR planned_start IS NULL OR planned_end >= planned_start),
    -- 审计价不加 CHECK：NULL / 0 / 负数一律按「未审定」处理（4.2.7），由应用层归一
    CHECK (audit_price_cent IS NULL OR audit_price_cent >= 0)
)
"#;

const CT_CONTRACT_ITEM: &str = r#"
CREATE TABLE IF NOT EXISTS ct_contract_item (
    id              TEXT PRIMARY KEY,
    contract_id     TEXT NOT NULL REFERENCES ct_contract(id) ON DELETE CASCADE,
    category_id     TEXT NOT NULL REFERENCES ct_category(id),
    item_code       TEXT,
    item_name       TEXT NOT NULL,
    unit            TEXT,
    quantity_x1000  INTEGER NOT NULL DEFAULT 0,
    unit_price_cent INTEGER NOT NULL DEFAULT 0,
    amount_cent     INTEGER NOT NULL DEFAULT 0 CHECK (amount_cent >= 0),
    sort_no         INTEGER NOT NULL DEFAULT 0,
    remark          TEXT,
    created_at      TEXT NOT NULL,
    created_by      TEXT,
    updated_at      TEXT NOT NULL,
    updated_by      TEXT,
    version         INTEGER NOT NULL DEFAULT 1
)
"#;

/// 合同附件（本期四类，每类一份，文件本体以 BLOB 入库）。
///
/// 选择 BLOB 而非「落盘 + 索引」的取舍：整套系统只有一个 `ctp.db`，
/// 备份与迁移不会出现「库走了、附件没走」；代价是库随附件增长，
/// 由应用层的 50 MB 单文件上限兜住。
const CT_CONTRACT_ATTACHMENT: &str = r#"
CREATE TABLE IF NOT EXISTS ct_contract_attachment (
    id          TEXT PRIMARY KEY,
    contract_id TEXT NOT NULL REFERENCES ct_contract(id) ON DELETE CASCADE,
    att_type    TEXT NOT NULL
                CHECK (att_type IN ('BID_NOTICE','CONTRACT','PROGRESS_SETTLEMENT','AUDIT_REPORT')),
    file_name   TEXT NOT NULL,
    mime_type   TEXT,
    size_bytes  INTEGER NOT NULL CHECK (size_bytes > 0),
    sha256      TEXT NOT NULL,
    content     BLOB NOT NULL,
    remark      TEXT,
    created_at  TEXT NOT NULL,
    created_by  TEXT,
    updated_at  TEXT NOT NULL,
    updated_by  TEXT,
    version     INTEGER NOT NULL DEFAULT 1
)
"#;

/// 拨款单（本期唯一支付表）。
/// 评审 H3：无 `application_id`；评审 H5：无 `reversal_of`，`status ∈ ('PAID','VOID')`。
const PAY_PAYMENT: &str = r#"
CREATE TABLE IF NOT EXISTS pay_payment (
    id              TEXT PRIMARY KEY,
    payment_no      TEXT NOT NULL,
    title           TEXT,
    sort_no         INTEGER NOT NULL DEFAULT 0,
    period_no       INTEGER NOT NULL DEFAULT 0 CHECK (period_no >= 0),
    contract_id     TEXT NOT NULL REFERENCES ct_contract(id),
    project_id      TEXT NOT NULL REFERENCES prj_project(id),
    pay_date        TEXT NOT NULL,
    amount_cent     INTEGER NOT NULL CHECK (amount_cent > 0),
    reason          TEXT,
    pay_method      TEXT NOT NULL DEFAULT 'TRANSFER'
                    CHECK (pay_method IN ('TRANSFER','ACCEPTANCE','CASH','OTHER')),
    bank_serial_no  TEXT,
    pay_account     TEXT,
    receive_account TEXT,
    status          TEXT NOT NULL DEFAULT 'PAID' CHECK (status IN ('PAID','VOID')),
    operator_id     TEXT NOT NULL,
    print_count     INTEGER NOT NULL DEFAULT 0 CHECK (print_count >= 0),
    remark          TEXT,
    is_deleted      INTEGER NOT NULL DEFAULT 0 CHECK (is_deleted IN (0,1)),
    created_at      TEXT NOT NULL,
    created_by      TEXT,
    updated_at      TEXT NOT NULL,
    updated_by      TEXT,
    version         INTEGER NOT NULL DEFAULT 1
)
"#;

/// 四个视图（V1～V4）。已付口径一律引用 `paid_net_subquery` 与 `PAID_STATUS_SQL`。
pub fn view_statements() -> Vec<(&'static str, String)> {
    let base = base_expr("c");
    let paid = "COALESCE(p.paid_cent, 0)";
    let v1 = format!(
        r#"
CREATE VIEW IF NOT EXISTS v_contract_summary AS
SELECT
    c.id                                            AS contract_id,
    c.project_id                                    AS project_id,
    c.contract_no                                   AS contract_no,
    c.contract_name                                 AS contract_name,
    c.status                                        AS status,
    o.contractor_name                               AS contractor_name,
    c.contract_category_code                        AS contract_category_code,
    cat.category_code                               AS category_code,
    cat.category_name                               AS category_name,
    c.original_amount_cent                          AS total_amount_cent,
    c.audit_price_cent                              AS audit_price_cent,
    CASE WHEN c.audit_price_cent > 0 THEN 1 ELSE 0 END AS is_audited,
    {base}                                          AS base_amount_cent,
    CASE WHEN c.audit_price_cent > 0
         THEN c.original_amount_cent - c.audit_price_cent ELSE 0 END AS audit_reduction_cent,
    {paid}                                          AS paid_cent,
    0                                               AS inflight_cent,
    {paid}                                          AS occupied_cent,
    {base} - {paid}                                 AS available_cent,
    {rate}                                          AS pay_rate_bp,
    CASE WHEN {base} < {paid} THEN 1 ELSE 0 END     AS is_inverted,
    c.planned_end                                   AS planned_end,
    c.duration_days                                 AS duration_days
FROM ct_contract c
JOIN org_contractor o   ON o.id = c.contractor_id
LEFT JOIN ct_category cat ON cat.id = c.category_id
LEFT JOIN ({paid_sub}) p ON p.contract_id = c.id
WHERE c.is_deleted = 0
"#,
        base = base,
        paid = paid,
        rate = rate_bp_expr(paid, &base),
        paid_sub = paid_net_subquery("pm.contract_id"),
    );

    let v2 = r#"
CREATE VIEW IF NOT EXISTS v_payment_ledger AS
SELECT
    pm.pay_date            AS pay_date,
    pm.payment_no          AS payment_no,
    pm.status              AS payment_status,
    pj.project_name        AS project_name,
    c.contract_no          AS contract_no,
    c.contract_name        AS contract_name,
    o.contractor_name      AS contractor_name,
    cat.category_name      AS category_name,
    pm.reason              AS reason,
    pm.amount_cent         AS amount_cent,
    SUM(pm.amount_cent) OVER (
        PARTITION BY pm.contract_id ORDER BY pm.pay_date, pm.payment_no
    )                      AS cum_amount_cent,
    cs.total_amount_cent   AS total_amount_cent,
    cs.audit_price_cent    AS audit_price_cent,
    cs.base_amount_cent    AS base_amount_cent,
    cs.is_audited          AS is_audited,
    cs.pay_rate_bp         AS pay_rate_bp
FROM pay_payment pm
JOIN ct_contract c       ON c.id = pm.contract_id
JOIN prj_project pj      ON pj.id = pm.project_id
JOIN org_contractor o    ON o.id = c.contractor_id
LEFT JOIN ct_category cat ON cat.id = c.category_id
LEFT JOIN v_contract_summary cs ON cs.contract_id = c.id
WHERE pm.is_deleted = 0 AND c.is_deleted = 0
  AND pm.status IN ('PAID')
"#
    .to_string();

    let v3 = format!(
        r#"
CREATE VIEW IF NOT EXISTS v_category_summary AS
WITH contract_amt AS (
    -- 未审定且有清单行：按清单行分摊到会计分类
    SELECT c.project_id AS project_id, ci.category_id AS category_id, c.id AS contract_id,
           SUM(ci.amount_cent) AS amount_cent, 0 AS audit_cent
    FROM ct_contract c
    JOIN ct_contract_item ci ON ci.contract_id = c.id
    WHERE c.is_deleted = 0 AND c.status <> 'VOID'
      AND (c.audit_price_cent IS NULL OR c.audit_price_cent <= 0)
    GROUP BY c.id, ci.category_id
    UNION ALL
    -- 未审定且无清单行：按合同级主导分类整笔归集合同金额
    SELECT c.project_id AS project_id, c.category_id AS category_id, c.id AS contract_id,
           c.original_amount_cent AS amount_cent, 0 AS audit_cent
    FROM ct_contract c
    WHERE c.is_deleted = 0 AND c.status <> 'VOID'
      AND (c.audit_price_cent IS NULL OR c.audit_price_cent <= 0)
      AND c.category_id IS NOT NULL
      AND NOT EXISTS (SELECT 1 FROM ct_contract_item ci WHERE ci.contract_id = c.id)
    UNION ALL
    -- 已审定：审计价整笔归集到主导分类，不与清单重复累加
    SELECT c.project_id AS project_id, c.category_id AS category_id, c.id AS contract_id,
           c.audit_price_cent AS amount_cent, c.audit_price_cent AS audit_cent
    FROM ct_contract c
    WHERE c.is_deleted = 0 AND c.status <> 'VOID'
      AND c.audit_price_cent > 0 AND c.category_id IS NOT NULL
),
amt AS (
    SELECT project_id, category_id,
           COUNT(DISTINCT contract_id) AS contract_count,
           SUM(amount_cent) AS base_amount_cent,
           SUM(audit_cent)  AS audit_amount_cent
    FROM contract_amt GROUP BY project_id, category_id
),
paid AS (
    SELECT c.project_id AS project_id, c.category_id AS category_id,
           SUM(pm.amount_cent) AS paid_cent
    FROM pay_payment pm
    JOIN ct_contract c ON c.id = pm.contract_id
    WHERE pm.is_deleted = 0 AND pm.status IN ({paid_status})
    GROUP BY c.project_id, c.category_id
)
SELECT
    amt.project_id                                  AS project_id,
    cat.id                                          AS category_id,
    cat.category_code                               AS category_code,
    cat.category_name                               AS category_name,
    amt.contract_count                              AS contract_count,
    amt.base_amount_cent                            AS base_amount_cent,
    amt.audit_amount_cent                           AS audit_amount_cent,
    COALESCE(paid.paid_cent, 0)                     AS paid_amount_cent,
    amt.base_amount_cent - COALESCE(paid.paid_cent, 0) AS unpaid_amount_cent,
    {rate}                                          AS pay_rate_bp
FROM amt
JOIN ct_category cat ON cat.id = amt.category_id
LEFT JOIN paid ON paid.project_id = amt.project_id AND paid.category_id = amt.category_id
"#,
        paid_status = PAID_STATUS_SQL,
        rate = rate_bp_expr("COALESCE(paid.paid_cent, 0)", "amt.base_amount_cent"),
    );

    let v4 = format!(
        r#"
CREATE VIEW IF NOT EXISTS v_payment_period AS
SELECT
    pm.project_id                      AS project_id,
    substr(pm.pay_date, 1, 7)          AS period_month,
    COUNT(*)                           AS payment_count,
    SUM(pm.amount_cent)                AS amount_cent
FROM pay_payment pm
WHERE pm.is_deleted = 0 AND pm.status IN ({paid_status})
GROUP BY pm.project_id, substr(pm.pay_date, 1, 7)
"#,
        paid_status = PAID_STATUS_SQL
    );

    vec![
        ("v_contract_summary", v1),
        ("v_payment_ledger", v2),
        ("v_category_summary", v3),
        ("v_payment_period", v4),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 比率表达式把乘二放在分母() {
        let e = rate_bp_expr("paid", "base");
        assert!(e.contains("(base * 2)")); 
        assert!(
            !e.contains("/ (base) * 2") && !e.contains("/ base * 2"),
            "不得出现除后乘 2：{e}"
        );
    }

    #[test]
    fn 已付子查询只取_paid_状态且直连_pay_payment() {
        let s = paid_net_subquery("pm.contract_id");
        assert!(s.contains("FROM pay_payment"));
        assert!(s.contains("status IN ('PAID')"));
        assert!(!s.contains("allocation"), "H2：不得再经分摊表");
    }

    #[test]
    fn 视图语句全部可拼装() {
        let views = view_statements();
        assert_eq!(views.len(), 4);
        for (name, sql) in views {
            assert!(sql.contains("CREATE VIEW IF NOT EXISTS"), "{name}");
            assert!(!sql.contains("/ base * 2"), "{name} 比率表达式仍有 H1 缺陷");
        }
    }

    #[test]
    fn 本期不建申请单与红冲相关结构() {
        let all: String = table_statements()
            .into_iter()
            .map(|(_, s)| s)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!all.contains("pay_application"));
        assert!(!all.contains("reversal_of"));
        assert!(all.contains("CHECK (status IN ('PAID','VOID'))"));
    }
}
