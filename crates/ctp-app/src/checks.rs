//! 口径与结构自检（`ctp selfcheck` 与界面「自检」页共用同一套规则）。
//!
//! 六项检查与评审结论一一对应：
//! 1. 支付比率 —— V1 视图与 `Rate::ratio` 逐位一致（H1）；
//! 2. DB-06 —— 业务表具备 5 个审计/版本列（M4）；
//! 3. 范围台账 —— P0 条目全部属于本期范围（M5）；
//! 4. 外键 —— 无悬空引用；
//! 5. 已付口径 —— V1 与「仅 PAID 合计」一致（H2 / H5）；
//! 6. 视图行数 —— `v_contract_summary` 行数与合同数一致（无 JOIN 丢行）。

use crate::service;
use ctp_core::scope;
use ctp_store::{repo, schema, Result};
use rusqlite::Connection;

/// 单项检查结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckItem {
    pub name: &'static str,
    pub passed: bool,
    /// 通过时给出规则说明，不通过时给出违例清单。
    pub detail: String,
}

impl CheckItem {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            passed: true,
            detail: detail.into(),
        }
    }
    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            passed: false,
            detail: detail.into(),
        }
    }
}

/// 自检总览。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub schema_version: String,
    pub items: Vec<CheckItem>,
}

impl CheckReport {
    pub fn total(&self) -> usize {
        self.items.len()
    }
    pub fn failed(&self) -> usize {
        self.items.iter().filter(|i| !i.passed).count()
    }
    pub fn all_passed(&self) -> bool {
        self.failed() == 0
    }
}

const N_RATE: &str = "支付比率（H1）";
const N_AUDIT: &str = "表结构 DB-06（M4）";
const N_PLAN: &str = "范围台账 P0（M5）";
const N_FK: &str = "外键完整性";
const N_PAID: &str = "已付口径（H2/H5）";
const N_ROWS: &str = "视图行数";

/// 跑一遍全部检查。单项异常不中断，转成该条的失败说明。
pub fn run(conn: &Connection) -> Result<CheckReport> {
    let version = ctp_store::db::schema_version(conn)?.unwrap_or_else(|| "（未初始化）".into());
    let mut items = Vec::with_capacity(6);

    match service::reconcile(conn) {
        Ok(m) if m.is_empty() => items.push(CheckItem::pass(
            N_RATE,
            "视图与 Rate::ratio 逐位一致（差异 0）",
        )),
        Ok(m) => {
            let detail = m
                .iter()
                .take(5)
                .map(|x| {
                    format!(
                        "{}：视图 {} bp ≠ 应用层 {} bp",
                        x.contract_no, x.view_bp, x.app_bp
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            items.push(CheckItem::fail(
                N_RATE,
                format!("{} 处不一致\n{detail}", m.len()),
            ));
        }
        Err(e) => items.push(CheckItem::fail(N_RATE, e.to_string())),
    }

    match repo::audit_column_violations(conn) {
        Ok(v) if v.is_empty() => items.push(CheckItem::pass(
            N_AUDIT,
            "业务表均具备 created_at/created_by/updated_at/updated_by/version（sys_audit_log 已豁免）",
        )),
        Ok(v) => items.push(CheckItem::fail(N_AUDIT, format!("缺列 {} 张表：{}", v.len(), v.join("、")))),
        Err(e) => items.push(CheckItem::fail(N_AUDIT, e.to_string())),
    }

    let plan_issues = scope::audit_p0_consistency();
    if plan_issues.is_empty() {
        items.push(CheckItem::pass(
            N_PLAN,
            format!("P0 共 {} 项，全部属于本期范围", scope::p0_items().count()),
        ));
    } else {
        items.push(CheckItem::fail(N_PLAN, plan_issues.join("\n")));
    }

    match foreign_key_violations(conn) {
        Ok(0) => items.push(CheckItem::pass(N_FK, "PRAGMA foreign_key_check 无悬空引用")),
        Ok(n) => items.push(CheckItem::fail(N_FK, format!("悬空引用 {n} 处"))),
        Err(e) => items.push(CheckItem::fail(N_FK, e)),
    }

    match paid_net_mismatch(conn) {
        Ok(0) => items.push(CheckItem::pass(
            N_PAID,
            "V1 已付与「仅 PAID 合计」完全一致（作废单未计入）",
        )),
        Ok(n) => items.push(CheckItem::fail(
            N_PAID,
            format!("{n} 份合同的已付口径不一致"),
        )),
        Err(e) => items.push(CheckItem::fail(N_PAID, e)),
    }

    match orphan_view_rows(conn) {
        Ok(0) => items.push(CheckItem::pass(
            N_ROWS,
            "视图行数 = 合同行数（无因 JOIN 丢行）",
        )),
        Ok(n) => items.push(CheckItem::fail(N_ROWS, format!("视图与合同相差 {n} 行"))),
        Err(e) => items.push(CheckItem::fail(N_ROWS, e)),
    }

    Ok(CheckReport {
        schema_version: version,
        items,
    })
}

fn foreign_key_violations(conn: &Connection) -> std::result::Result<i64, String> {
    let mut stmt = conn
        .prepare("PRAGMA foreign_key_check")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut n = 0i64;
    while rows.next().map_err(|e| e.to_string())?.is_some() {
        n += 1;
    }
    Ok(n)
}

/// 视图已付 vs 独立重算的已付（只取 PAID），不等即为违例。
fn paid_net_mismatch(conn: &Connection) -> std::result::Result<i64, String> {
    let sql = format!(
        "SELECT COUNT(*) FROM ( \
            SELECT cs.contract_id, cs.paid_cent AS view_paid, \
                   COALESCE((SELECT SUM(amount_cent) FROM pay_payment p \
                             WHERE p.contract_id = cs.contract_id AND p.is_deleted = 0 \
                               AND p.status IN ({paid})), 0) AS expected \
            FROM v_contract_summary cs \
         ) WHERE view_paid <> expected",
        paid = schema::PAID_STATUS_SQL
    );
    conn.query_row(&sql, [], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())
}

fn orphan_view_rows(conn: &Connection) -> std::result::Result<i64, String> {
    let contracts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ct_contract WHERE is_deleted = 0",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let views: i64 = conn
        .query_row("SELECT COUNT(*) FROM v_contract_summary", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok((contracts - views).abs())
}
