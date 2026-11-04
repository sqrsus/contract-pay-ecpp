use crate::error::{Result, StoreError};
use crate::schema;
use ctp_core::contract::{Contract, ContractStatus, ContractType, PricingMethod};
use ctp_core::date::now_iso;
use ctp_core::ledger::{CategoryRow, ContractSummary, LedgerRow, PeriodRow};
use ctp_core::money::Money;
use ctp_core::payment::{PayMethod, Payment, PaymentStatus};
use ctp_core::project::{Contact, Contractor, Project};
use ctp_core::rate::Rate;
use rusqlite::{Connection, Row};

fn conv<T>(v: Option<T>, field: &str) -> rusqlite::Result<T> {
    v.ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(StoreError::Inconsistent(format!("字段 {field} 取值非法"))),
        )
    })
}

fn money(v: i64) -> Money {
    Money::from_cent(v)
}

// ---------------------------------------------------------------- 基础档案

pub fn insert_project(conn: &Connection, p: &Project) -> Result<()> {
    p.validate()?;
    conn.execute(
        "INSERT INTO prj_project(id, project_code, project_name, remark, is_deleted, \
            created_at, created_by, updated_at, updated_by, version) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?6,?7,?8)",
        rusqlite::params![
            p.id,
            p.project_code,
            p.project_name,
            p.remark,
            p.is_deleted as i32,
            p.created_at,
            p.created_by,
            p.version
        ],
    )?;
    Ok(())
}

pub fn insert_contractor(conn: &Connection, c: &Contractor) -> Result<()> {
    c.validate()?;
    conn.execute(
        "INSERT INTO org_contractor(id, contractor_name, role_type, credit_code, bank_name, \
            bank_account, remark, is_deleted, created_at, created_by, updated_at, updated_by, version) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?9,?10,?11)",
        rusqlite::params![
            c.id,
            c.contractor_name,
            c.role_type.code(),
            c.credit_code,
            c.bank_name,
            c.bank_account,
            c.remark,
            c.is_deleted as i32,
            c.created_at,
            c.created_by,
            c.version
        ],
    )?;
    Ok(())
}

pub fn insert_contact(conn: &Connection, c: &Contact) -> Result<()> {
    c.validate()?;
    conn.execute(
        "INSERT INTO org_contact(id, contractor_id, contact_name, phone, is_primary, \
            created_at, created_by, updated_at, updated_by, version) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?6,?7,?8)",
        rusqlite::params![
            c.id,
            c.contractor_id,
            c.contact_name,
            c.phone,
            c.is_primary as i32,
            c.created_at,
            c.created_by,
            c.version
        ],
    )?;
    Ok(())
}

pub fn insert_category(
    conn: &Connection,
    id: &str,
    code: &str,
    name: &str,
    sort_no: i64,
    operator: &str,
) -> Result<()> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO ct_category(id, category_code, category_name, sort_no, is_enabled, \
            created_at, created_by, updated_at, updated_by, version) \
         VALUES (?1,?2,?3,?4,1,?5,?6,?5,?6,1) \
         ON CONFLICT(category_code) DO UPDATE SET category_name = excluded.category_name",
        rusqlite::params![id, code, name, sort_no, now, operator],
    )?;
    Ok(())
}

pub fn insert_dict_item(
    conn: &Connection,
    dict_type: &str,
    code: &str,
    name: &str,
    sort_no: i64,
    operator: &str,
) -> Result<()> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO sys_dict_item(dict_type, item_code, item_name, sort_no, is_enabled, \
            created_at, created_by, updated_at, updated_by, version) \
         VALUES (?1,?2,?3,?4,1,?5,?6,?5,?6,1) \
         ON CONFLICT(dict_type, item_code) DO UPDATE SET item_name = excluded.item_name",
        rusqlite::params![dict_type, code, name, sort_no, now, operator],
    )?;
    Ok(())
}

pub fn category_exists(conn: &Connection, id: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ct_category WHERE id = ?1",
        [id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

// ---------------------------------------------------------------- 合同

const CONTRACT_COLS: &str = "id, project_id, contract_no, contract_name, contract_type, \
contract_category_code, category_id, party_a, contractor_id, building_id, sort_no, sign_date, \
original_amount_cent, audit_price_cent, audit_date, audit_org, audit_report_no, duration_days, \
planned_start, planned_end, actual_start, actual_end, pricing_method, retention_rate_bp, \
advance_rate_bp, status, payment_terms, remark, is_deleted, version, created_at, created_by, \
updated_at, updated_by";

fn map_contract(row: &Row) -> rusqlite::Result<Contract> {
    Ok(Contract {
        id: row.get(0)?,
        project_id: row.get(1)?,
        contract_no: row.get(2)?,
        contract_name: row.get(3)?,
        contract_type: conv(
            ContractType::from_code(&row.get::<_, String>(4)?),
            "contract_type",
        )?,
        contract_category_code: row.get(5)?,
        category_id: row.get(6)?,
        party_a: row.get(7)?,
        contractor_id: row.get(8)?,
        building_id: row.get(9)?,
        sort_no: row.get(10)?,
        sign_date: row.get(11)?,
        original_amount_cent: money(row.get(12)?),
        audit_price_cent: row.get::<_, Option<i64>>(13)?.map(money),
        audit_date: row.get(14)?,
        audit_org: row.get(15)?,
        audit_report_no: row.get(16)?,
        duration_days: row.get(17)?,
        planned_start: row.get(18)?,
        planned_end: row.get(19)?,
        actual_start: row.get(20)?,
        actual_end: row.get(21)?,
        pricing_method: match row.get::<_, Option<String>>(22)? {
            Some(s) => Some(conv(PricingMethod::from_code(&s), "pricing_method")?),
            None => None,
        },
        retention_rate_bp: row.get(23)?,
        advance_rate_bp: row.get(24)?,
        status: conv(ContractStatus::from_code(&row.get::<_, String>(25)?), "status")?,
        payment_terms: row.get(26)?,
        remark: row.get(27)?,
        is_deleted: row.get::<_, i32>(28)? != 0,
        version: row.get(29)?,
        created_at: row.get(30)?,
        created_by: row.get(31)?,
        updated_at: row.get(32)?,
        updated_by: row.get(33)?,
    })
}

pub fn insert_contract(conn: &Connection, c: &Contract) -> Result<()> {
    c.validate()?;
    conn.execute(
        "INSERT INTO ct_contract(id, project_id, contract_no, contract_name, contract_type, \
            contract_category_code, category_id, party_a, contractor_id, building_id, sort_no, \
            sign_date, original_amount_cent, audit_price_cent, audit_date, audit_org, \
            audit_report_no, duration_days, planned_start, planned_end, actual_start, actual_end, \
            pricing_method, retention_rate_bp, advance_rate_bp, status, payment_terms, remark, \
            is_deleted, created_at, created_by, updated_at, updated_by, version) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34)",
        rusqlite::params![
            c.id,
            c.project_id,
            c.contract_no,
            c.contract_name,
            c.contract_type.code(),
            c.contract_category_code,
            c.category_id,
            c.party_a,
            c.contractor_id,
            c.building_id,
            c.sort_no,
            c.sign_date,
            c.original_amount_cent.cent(),
            c.audit_price_cent.map(|m| m.cent()),
            c.audit_date,
            c.audit_org,
            c.audit_report_no,
            c.duration_days,
            c.planned_start,
            c.planned_end,
            c.actual_start,
            c.actual_end,
            c.pricing_method.map(|p| p.code()),
            c.retention_rate_bp,
            c.advance_rate_bp,
            c.status.code(),
            c.payment_terms,
            c.remark,
            c.is_deleted as i32,
            c.created_at,
            c.created_by,
            c.updated_at,
            c.updated_by,
            c.version
        ],
    )?;
    Ok(())
}

/// 全量更新（乐观锁：`version` 不匹配即报冲突）。
pub fn update_contract(conn: &Connection, c: &Contract) -> Result<()> {
    c.validate()?;
    let n = conn.execute(
        "UPDATE ct_contract SET \
            project_id=?3, contract_no=?4, contract_name=?5, contract_type=?6, \
            contract_category_code=?7, category_id=?8, party_a=?9, contractor_id=?10, \
            building_id=?11, sort_no=?12, sign_date=?13, original_amount_cent=?14, \
            audit_price_cent=?15, audit_date=?16, audit_org=?17, audit_report_no=?18, \
            duration_days=?19, planned_start=?20, planned_end=?21, actual_start=?22, actual_end=?23, \
            pricing_method=?24, retention_rate_bp=?25, advance_rate_bp=?26, status=?27, \
            payment_terms=?28, remark=?29, is_deleted=?30, updated_at=?31, updated_by=?32, \
            version = version + 1 \
         WHERE id=?1 AND version=?2",
        rusqlite::params![
            c.id,
            c.version,
            c.project_id,
            c.contract_no,
            c.contract_name,
            c.contract_type.code(),
            c.contract_category_code,
            c.category_id,
            c.party_a,
            c.contractor_id,
            c.building_id,
            c.sort_no,
            c.sign_date,
            c.original_amount_cent.cent(),
            c.audit_price_cent.map(|m| m.cent()),
            c.audit_date,
            c.audit_org,
            c.audit_report_no,
            c.duration_days,
            c.planned_start,
            c.planned_end,
            c.actual_start,
            c.actual_end,
            c.pricing_method.map(|p| p.code()),
            c.retention_rate_bp,
            c.advance_rate_bp,
            c.status.code(),
            c.payment_terms,
            c.remark,
            c.is_deleted as i32,
            c.updated_at,
            c.updated_by
        ],
    )?;
    if n == 0 {
        return Err(StoreError::Inconsistent(format!(
            "合同 {} 更新失败：记录不存在或已被并发修改（期望版本 {}）",
            c.id, c.version
        )));
    }
    Ok(())
}

pub fn get_contract(conn: &Connection, id: &str) -> Result<Contract> {
    let sql = format!("SELECT {CONTRACT_COLS} FROM ct_contract WHERE id = ?1");
    conn.query_row(&sql, [id], map_contract)
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(format!("合同 {id}")),
            other => StoreError::Sql(other),
        })
}

pub fn find_contract_by_no(conn: &Connection, no: &str) -> Result<Option<Contract>> {
    use rusqlite::OptionalExtension;
    let sql = format!(
        "SELECT {CONTRACT_COLS} FROM ct_contract WHERE contract_no = ?1 AND is_deleted = 0"
    );
    Ok(conn.query_row(&sql, [no], map_contract).optional()?)
}

pub fn list_contracts(conn: &Connection) -> Result<Vec<Contract>> {
    let sql = format!("SELECT {CONTRACT_COLS} FROM ct_contract WHERE is_deleted = 0 ORDER BY contract_no");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_contract)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn contract_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM ct_contract WHERE is_deleted = 0",
        [],
        |r| r.get(0),
    )?)
}

/// 项目清单（供界面下拉选择）。
pub fn list_projects(conn: &Connection) -> Result<Vec<Project>> {
    let sql = "SELECT id, project_code, project_name, remark, is_deleted, version, \
        created_at, created_by, updated_at, updated_by \
        FROM prj_project WHERE is_deleted = 0 ORDER BY project_code";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(Project {
            id: row.get(0)?,
            project_code: row.get(1)?,
            project_name: row.get(2)?,
            remark: row.get(3)?,
            is_deleted: row.get::<_, i64>(4)? != 0,
            version: row.get(5)?,
            created_at: row.get(6)?,
            created_by: row.get(7)?,
            updated_at: row.get(8)?,
            updated_by: row.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 参建单位清单（供界面下拉选择）。
pub fn list_contractors(conn: &Connection) -> Result<Vec<Contractor>> {
    let sql = "SELECT id, contractor_name, role_type, credit_code, bank_name, bank_account, remark, \
        is_deleted, version, created_at, created_by, updated_at, updated_by \
        FROM org_contractor WHERE is_deleted = 0 ORDER BY contractor_name";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        let role: String = row.get(2)?;
        Ok(Contractor {
            id: row.get(0)?,
            contractor_name: row.get(1)?,
            role_type: ctp_core::project::ContractorRole::from_code(&role)
                .unwrap_or(ctp_core::project::ContractorRole::Other),
            credit_code: row.get(3)?,
            bank_name: row.get(4)?,
            bank_account: row.get(5)?,
            remark: row.get(6)?,
            is_deleted: row.get::<_, i64>(7)? != 0,
            version: row.get(8)?,
            created_at: row.get(9)?,
            created_by: row.get(10)?,
            updated_at: row.get(11)?,
            updated_by: row.get(12)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ---------------------------------------------------------------- 拨款单

const PAYMENT_COLS: &str = "id, payment_no, title, sort_no, period_no, contract_id, project_id, \
pay_date, amount_cent, reason, pay_method, bank_serial_no, pay_account, receive_account, status, \
operator_id, print_count, remark, is_deleted, version, created_at, created_by, updated_at, updated_by";

fn map_payment(row: &Row) -> rusqlite::Result<Payment> {
    Ok(Payment {
        id: row.get(0)?,
        payment_no: row.get(1)?,
        title: row.get(2)?,
        sort_no: row.get(3)?,
        period_no: row.get(4)?,
        contract_id: row.get(5)?,
        project_id: row.get(6)?,
        pay_date: row.get(7)?,
        amount_cent: money(row.get(8)?),
        reason: row.get(9)?,
        pay_method: conv(PayMethod::from_code(&row.get::<_, String>(10)?), "pay_method")?,
        bank_serial_no: row.get(11)?,
        pay_account: row.get(12)?,
        receive_account: row.get(13)?,
        status: conv(PaymentStatus::from_code(&row.get::<_, String>(14)?), "status")?,
        operator_id: row.get(15)?,
        print_count: row.get(16)?,
        remark: row.get(17)?,
        is_deleted: row.get::<_, i32>(18)? != 0,
        version: row.get(19)?,
        created_at: row.get(20)?,
        created_by: row.get(21)?,
        updated_at: row.get(22)?,
        updated_by: row.get(23)?,
    })
}

pub fn insert_payment(conn: &Connection, p: &Payment) -> Result<()> {
    p.validate()?;
    conn.execute(
        "INSERT INTO pay_payment(id, payment_no, title, sort_no, period_no, contract_id, \
            project_id, pay_date, amount_cent, reason, pay_method, bank_serial_no, pay_account, \
            receive_account, status, operator_id, print_count, remark, is_deleted, \
            created_at, created_by, updated_at, updated_by, version) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)",
        rusqlite::params![
            p.id,
            p.payment_no,
            p.title,
            p.sort_no,
            p.period_no,
            p.contract_id,
            p.project_id,
            p.pay_date,
            p.amount_cent.cent(),
            p.reason,
            p.pay_method.code(),
            p.bank_serial_no,
            p.pay_account,
            p.receive_account,
            p.status.code(),
            p.operator_id,
            p.print_count,
            p.remark,
            p.is_deleted as i32,
            p.created_at,
            p.created_by,
            p.updated_at,
            p.updated_by,
            p.version
        ],
    )?;
    Ok(())
}

pub fn update_payment(conn: &Connection, p: &Payment) -> Result<()> {
    let n = conn.execute(
        "UPDATE pay_payment SET title=?3, sort_no=?4, period_no=?5, contract_id=?6, project_id=?7, \
            pay_date=?8, amount_cent=?9, reason=?10, pay_method=?11, bank_serial_no=?12, \
            pay_account=?13, receive_account=?14, status=?15, operator_id=?16, print_count=?17, \
            remark=?18, is_deleted=?19, updated_at=?20, updated_by=?21, version = version + 1 \
         WHERE id=?1 AND version=?2",
        rusqlite::params![
            p.id,
            p.version,
            p.title,
            p.sort_no,
            p.period_no,
            p.contract_id,
            p.project_id,
            p.pay_date,
            p.amount_cent.cent(),
            p.reason,
            p.pay_method.code(),
            p.bank_serial_no,
            p.pay_account,
            p.receive_account,
            p.status.code(),
            p.operator_id,
            p.print_count,
            p.remark,
            p.is_deleted as i32,
            p.updated_at,
            p.updated_by
        ],
    )?;
    if n == 0 {
        return Err(StoreError::Inconsistent(format!(
            "拨款单 {} 更新失败：记录不存在或已被并发修改",
            p.id
        )));
    }
    Ok(())
}

pub fn get_payment(conn: &Connection, id: &str) -> Result<Payment> {
    let sql = format!("SELECT {PAYMENT_COLS} FROM pay_payment WHERE id = ?1");
    conn.query_row(&sql, [id], map_payment)
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(format!("拨款单 {id}")),
            other => StoreError::Sql(other),
        })
}

/// 已付净额——**唯一取数实现**（评审 H2）。
///
/// 直接复用 `schema::paid_net_subquery`（与视图 V1～V4 同一段 SQL、同一状态集合），
/// 不再经 `pay_payment_allocation`，也不再有任何「例外口径」。
pub fn paid_net(conn: &Connection, contract_id: &str) -> Result<Money> {
    let sql = format!(
        "SELECT COALESCE(SUM(paid_cent), 0) FROM ({}) WHERE contract_id = ?1",
        schema::paid_net_subquery("pm.contract_id")
    );
    let v: i64 = conn.query_row(&sql, [contract_id], |r| r.get(0))?;
    Ok(money(v))
}

pub fn paid_payment_count(conn: &Connection, contract_id: &str) -> Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) FROM pay_payment WHERE contract_id = ?1 AND is_deleted = 0 AND status IN ({})",
        schema::PAID_STATUS_SQL
    );
    Ok(conn.query_row(&sql, [contract_id], |r| r.get(0))?)
}

pub fn list_payments(conn: &Connection, contract_id: Option<&str>) -> Result<Vec<Payment>> {
    let mut out = Vec::new();
    match contract_id {
        Some(cid) => {
            let sql = format!(
                "SELECT {PAYMENT_COLS} FROM pay_payment WHERE contract_id = ?1 AND is_deleted = 0 \
                 ORDER BY pay_date, payment_no"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([cid], map_payment)?;
            for r in rows {
                out.push(r?);
            }
        }
        None => {
            let sql = format!(
                "SELECT {PAYMENT_COLS} FROM pay_payment WHERE is_deleted = 0 ORDER BY pay_date, payment_no"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], map_payment)?;
            for r in rows {
                out.push(r?);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- 视图查询

fn map_summary(row: &Row) -> rusqlite::Result<ContractSummary> {
    Ok(ContractSummary {
        contract_id: row.get(0)?,
        project_id: row.get(1)?,
        contract_no: row.get(2)?,
        contract_name: row.get(3)?,
        status: row.get(4)?,
        contractor_name: row.get(5)?,
        contract_category_code: row.get(6)?,
        category_code: row.get(7)?,
        category_name: row.get(8)?,
        total_amount: money(row.get(9)?),
        audit_price: row.get::<_, Option<i64>>(10)?.map(money),
        is_audited: row.get::<_, i64>(11)? != 0,
        base_amount: money(row.get(12)?),
        audit_reduction: money(row.get(13)?),
        paid_amount: money(row.get(14)?),
        inflight_amount: money(row.get(15)?),
        occupied_amount: money(row.get(16)?),
        available_amount: money(row.get(17)?),
        pay_rate: Rate::from_bp(row.get(18)?),
        is_inverted: row.get::<_, i64>(19)? != 0,
        planned_end: row.get(20)?,
        duration_days: row.get(21)?,
    })
}

const SUMMARY_COLS: &str = "contract_id, project_id, contract_no, contract_name, status, \
contractor_name, contract_category_code, category_code, category_name, total_amount_cent, \
audit_price_cent, is_audited, base_amount_cent, audit_reduction_cent, paid_cent, inflight_cent, \
occupied_cent, available_cent, pay_rate_bp, is_inverted, planned_end, duration_days";

pub fn summaries(conn: &Connection) -> Result<Vec<ContractSummary>> {
    let sql = format!("SELECT {SUMMARY_COLS} FROM v_contract_summary ORDER BY contract_no");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_summary)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn summary(conn: &Connection, contract_id: &str) -> Result<Option<ContractSummary>> {
    use rusqlite::OptionalExtension;
    let sql = format!("SELECT {SUMMARY_COLS} FROM v_contract_summary WHERE contract_id = ?1");
    Ok(conn.query_row(&sql, [contract_id], map_summary).optional()?)
}

pub fn ledger(conn: &Connection, project_id: Option<&str>) -> Result<Vec<LedgerRow>> {
    let mut out = Vec::new();
    let sql = "SELECT pay_date, payment_no, payment_status, project_name, contract_no, \
        contract_name, contractor_name, category_name, reason, amount_cent, cum_amount_cent, \
        total_amount_cent, audit_price_cent, base_amount_cent, is_audited, pay_rate_bp \
        FROM v_payment_ledger";
    let map = |row: &Row| -> rusqlite::Result<LedgerRow> {
        Ok(LedgerRow {
            pay_date: row.get(0)?,
            payment_no: row.get(1)?,
            payment_status: row.get(2)?,
            project_name: row.get(3)?,
            contract_no: row.get(4)?,
            contract_name: row.get(5)?,
            contractor_name: row.get(6)?,
            category_name: row.get(7)?,
            reason: row.get(8)?,
            amount: money(row.get(9)?),
            cum_amount: money(row.get(10)?),
            total_amount: money(row.get(11)?),
            audit_price: row.get::<_, Option<i64>>(12)?.map(money),
            base_amount: money(row.get(13)?),
            is_audited: row.get::<_, i64>(14)? != 0,
            pay_rate: Rate::from_bp(row.get(15)?),
        })
    };
    match project_id {
        Some(pid) => {
            let mut stmt = conn.prepare(&format!("{sql} WHERE project_id = ?1 ORDER BY pay_date, payment_no"))?;
            let rows = stmt.query_map([pid], map)?;
            for r in rows {
                out.push(r?);
            }
        }
        None => {
            let mut stmt = conn.prepare(&format!("{sql} ORDER BY pay_date, payment_no"))?;
            let rows = stmt.query_map([], map)?;
            for r in rows {
                out.push(r?);
            }
        }
    }
    Ok(out)
}

pub fn category_summary(conn: &Connection) -> Result<Vec<CategoryRow>> {
    let sql = "SELECT project_id, category_id, category_code, category_name, contract_count, \
        base_amount_cent, audit_amount_cent, paid_amount_cent, unpaid_amount_cent, pay_rate_bp \
        FROM v_category_summary ORDER BY category_code";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(CategoryRow {
            project_id: row.get(0)?,
            category_id: row.get(1)?,
            category_code: row.get(2)?,
            category_name: row.get(3)?,
            contract_count: row.get(4)?,
            base_amount: money(row.get(5)?),
            audit_amount: money(row.get(6)?),
            paid_amount: money(row.get(7)?),
            unpaid_amount: money(row.get(8)?),
            pay_rate: Rate::from_bp(row.get(9)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn period_summary(conn: &Connection) -> Result<Vec<PeriodRow>> {
    let sql = "SELECT project_id, period_month, payment_count, amount_cent \
        FROM v_payment_period ORDER BY period_month";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(PeriodRow {
            project_id: row.get(0)?,
            period_month: row.get(1)?,
            payment_count: row.get(2)?,
            amount: money(row.get(3)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 列出所有合同及其视图比率，供 `selfcheck` 断言「视图 = 应用层」。
pub fn summary_with_expected_rate(conn: &Connection) -> Result<Vec<(String, i32, i32)>> {
    let rows = summaries(conn)?;
    Ok(rows
        .into_iter()
        .map(|s| {
            let expected = Rate::ratio(s.paid_amount, s.base_amount).bp();
            (s.contract_no, s.pay_rate.bp(), expected)
        })
        .collect())
}

/// 表结构自检：返回不满足 DB-06 的表清单（评审 M4）。
pub fn audit_column_violations(conn: &Connection) -> Result<Vec<String>> {
    let required = ["created_at", "created_by", "updated_at", "updated_by", "version"];
    let mut tables: Vec<String> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )?;
        let it = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for t in it {
            tables.push(t?);
        }
    }
    let mut bad = Vec::new();
    for t in tables {
        if schema::AUDIT_COLUMN_EXEMPT.contains(&t.as_str()) {
            continue;
        }
        let mut cols: Vec<String> = Vec::new();
        {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({t})"))?;
            let it = stmt.query_map([], |r| r.get::<_, String>(1))?;
            for c in it {
                cols.push(c?);
            }
        }
        let missing: Vec<&str> = required
            .iter()
            .copied()
            .filter(|c| !cols.iter().any(|x| x == c))
            .collect();
        if !missing.is_empty() {
            bad.push(format!("{t} 缺列 {missing:?}"));
        }
    }
    Ok(bad)
}
