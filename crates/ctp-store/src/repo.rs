use crate::error::{Result, StoreError};
use crate::schema;
use ctp_core::contract::{BidMethod, Contract, ContractStatus, ContractType, PricingMethod};
use ctp_core::date::now_iso;
use ctp_core::ledger::{
    CategoryRow, ContractSummary, LedgerRow, PeriodRow, SummaryDim, SummaryFilter, SummaryRow,
};
use ctp_core::money::Money;
use ctp_core::payment::{PayMethod, Payment, PaymentStatus};
use ctp_core::project::{Contact, Contractor, Project};
use ctp_core::rate::Rate;
use rusqlite::{Connection, OptionalExtension, Row};

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
updated_at, updated_by, bid_method, open_bid_date, project_leader, contact_info, bid_agent_id, \
delivery_date";

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
        status: conv(
            ContractStatus::from_code(&row.get::<_, String>(25)?),
            "status",
        )?,
        payment_terms: row.get(26)?,
        remark: row.get(27)?,
        is_deleted: row.get::<_, i32>(28)? != 0,
        version: row.get(29)?,
        created_at: row.get(30)?,
        created_by: row.get(31)?,
        updated_at: row.get(32)?,
        updated_by: row.get(33)?,
        // R-68（C3）：未知编码包成 BidMethod::Unknown 原样带回，不再静默变 None
        bid_method: row
            .get::<_, Option<String>>(34)?
            .map(|s| BidMethod::from_code_keep(&s)),
        open_bid_date: row.get(35)?,
        project_leader: row.get(36)?,
        contact_info: row.get(37)?,
        bid_agent_id: row.get(38)?,
        delivery_date: row.get(39)?,
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
            is_deleted, created_at, created_by, updated_at, updated_by, version, \
            bid_method, open_bid_date, project_leader, contact_info, bid_agent_id, delivery_date) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38,?39,?40)",
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
            c.version,
            c.bid_method.as_ref().map(|b| b.code()),
            c.open_bid_date,
            c.project_leader,
            c.contact_info,
            c.bid_agent_id,
            c.delivery_date
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
            bid_method=?33, open_bid_date=?34, project_leader=?35, contact_info=?36, bid_agent_id=?37, \
            delivery_date=?38, \
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
            c.updated_by,
            c.bid_method.as_ref().map(|b| b.code()),
            c.open_bid_date,
            c.project_leader,
            c.contact_info,
            c.bid_agent_id,
            c.delivery_date
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
    let sql = format!(
        "SELECT {CONTRACT_COLS} FROM ct_contract WHERE is_deleted = 0 ORDER BY contract_no"
    );
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
    let sql =
        "SELECT id, contractor_name, role_type, credit_code, bank_name, bank_account, remark, \
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
operator_id, remark, is_deleted, version, created_at, created_by, updated_at, updated_by";

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
        pay_method: conv(
            PayMethod::from_code(&row.get::<_, String>(10)?),
            "pay_method",
        )?,
        bank_serial_no: row.get(11)?,
        pay_account: row.get(12)?,
        receive_account: row.get(13)?,
        status: conv(
            PaymentStatus::from_code(&row.get::<_, String>(14)?),
            "status",
        )?,
        operator_id: row.get(15)?,
        remark: row.get(16)?,
        is_deleted: row.get::<_, i32>(17)? != 0,
        version: row.get(18)?,
        created_at: row.get(19)?,
        created_by: row.get(20)?,
        updated_at: row.get(21)?,
        updated_by: row.get(22)?,
    })
}

pub fn insert_payment(conn: &Connection, p: &Payment) -> Result<()> {
    p.validate()?;
    conn.execute(
        "INSERT INTO pay_payment(id, payment_no, title, sort_no, period_no, contract_id, \
            project_id, pay_date, amount_cent, reason, pay_method, bank_serial_no, pay_account, \
            receive_account, status, operator_id, remark, is_deleted, \
            created_at, created_by, updated_at, updated_by, version) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
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
    // 与 insert_payment 同口径：作废 / 编辑等任何回写路径都先过实体校验，
    // 否则金额被改成 0、日期非法、项目 id 清空也能写进库。
    p.validate()?;
    let n = conn.execute(
        "UPDATE pay_payment SET title=?3, sort_no=?4, period_no=?5, contract_id=?6, project_id=?7, \
            pay_date=?8, amount_cent=?9, reason=?10, pay_method=?11, bank_serial_no=?12, \
            pay_account=?13, receive_account=?14, status=?15, operator_id=?16, \
            remark=?17, is_deleted=?18, updated_at=?19, updated_by=?20, version = version + 1 \
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

/// 某张拨款单「本次之前」的累计已付（付款打印件 `已付款改成元` 的唯一取数实现）。
///
/// 排序口径与 V2 `v_payment_ledger` 的窗口函数一致：`(pay_date, payment_no)` 严格小于
/// 本单；只计已实付、未逻辑删除的单据，作废单不计入。
pub fn paid_before_payment(
    conn: &Connection,
    contract_id: &str,
    pay_date: &str,
    payment_no: &str,
) -> Result<Money> {
    let sql = format!(
        "SELECT COALESCE(SUM(amount_cent), 0) FROM pay_payment \
         WHERE contract_id = ?1 AND is_deleted = 0 AND status IN ({}) \
           AND (pay_date < ?2 OR (pay_date = ?2 AND payment_no < ?3))",
        schema::PAID_STATUS_SQL
    );
    let v: i64 = conn.query_row(
        &sql,
        rusqlite::params![contract_id, pay_date, payment_no],
        |r| r.get(0),
    )?;
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
        contract_type: row.get(6)?,
        contract_category_code: row.get(7)?,
        category_code: row.get(8)?,
        category_name: row.get(9)?,
        total_amount: money(row.get(10)?),
        audit_price: row.get::<_, Option<i64>>(11)?.map(money),
        is_audited: row.get::<_, i64>(12)? != 0,
        base_amount: money(row.get(13)?),
        audit_reduction: money(row.get(14)?),
        paid_amount: money(row.get(15)?),
        inflight_amount: money(row.get(16)?),
        occupied_amount: money(row.get(17)?),
        available_amount: money(row.get(18)?),
        pay_rate: Rate::from_bp(row.get(19)?),
        is_inverted: row.get::<_, i64>(20)? != 0,
        planned_end: row.get(21)?,
        duration_days: row.get(22)?,
        delivery_date: row.get(23)?,
    })
}

const SUMMARY_COLS: &str = "contract_id, project_id, contract_no, contract_name, status, \
contractor_name, contract_type, contract_category_code, category_code, category_name, \
total_amount_cent, audit_price_cent, is_audited, base_amount_cent, audit_reduction_cent, paid_cent, \
inflight_cent, occupied_cent, available_cent, pay_rate_bp, is_inverted, planned_end, duration_days, \
delivery_date";

pub fn summaries(conn: &Connection) -> Result<Vec<ContractSummary>> {
    let sql = format!("SELECT {SUMMARY_COLS} FROM v_contract_summary ORDER BY contract_no");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_summary)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn summary(conn: &Connection, contract_id: &str) -> Result<Option<ContractSummary>> {
    use rusqlite::OptionalExtension;
    let sql = format!("SELECT {SUMMARY_COLS} FROM v_contract_summary WHERE contract_id = ?1");
    Ok(conn
        .query_row(&sql, [contract_id], map_summary)
        .optional()?)
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
            let mut stmt = conn.prepare(&format!(
                "{sql} WHERE project_id = ?1 ORDER BY pay_date, payment_no"
            ))?;
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

/// 汇总统计：按维度 + 筛选条件归集已实付单据（作废单不计，R-32）。
///
/// 四个维度共用同一段 FROM / WHERE（`status='PAID' AND is_deleted=0`、日期闭区间、
/// 状态 / 类型 / 关键词筛选），只换分组键，保证切换维度后合计不会变。
pub fn payment_summary(
    conn: &Connection,
    dim: SummaryDim,
    f: &SummaryFilter,
) -> Result<Vec<SummaryRow>> {
    use SummaryDim as D;
    // 分组键 / 显示名；「按合同」额外带一列承包单位。
    // 档案缺失时显示「—」而不是回退成主键或空串（P7）：报表里的 `PRJ-2026-001`
    // 看着像项目编码，容易被当成名称；空串则与「未选」混在一起。
    let (key_expr, label_expr, sub_expr) = match dim {
        D::Project => (
            // R-68（C11）：项目维度按合同的当前项目归集，与 v_payment_period 同口径；
            // p.project_id 是签约快照，合同改项目后两处会给出不同答案
            "c.project_id",
            "COALESCE(pj.project_name, '—')".to_string(),
            "NULL",
        ),
        D::Contractor => (
            "c.contractor_id",
            "COALESCE(o.contractor_name, '—')".to_string(),
            "NULL",
        ),
        D::Contract => (
            "p.contract_id",
            "c.contract_no || ' · ' || c.contract_name".to_string(),
            "COALESCE(o.contractor_name, '—')",
        ),
        D::ContractType => ("c.contract_type", "c.contract_type".to_string(), "NULL"),
    };
    // 归档口径 = 合同全周期支付率 ≥ 100%，与 ContractSummary::is_archived 同源
    // （cs 是全周期视图，不受本查询的日期筛选影响）
    let archived = match f.archived {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    };
    // 项目名称与分组键必须同口径：分组按 c.project_id（合同当前项目），名称也要
    // JOIN 到 c.project_id。用单据上的签约快照 p.project_id 会在合同换项目后
    // 「归到新项目、显示旧项目名」；四个维度共用这段 WHERE，关键词同理受影响。
    // 与 v_payment_ledger / v_payment_period 的 C11 口径保持一致。
    let sql = format!(
        "SELECT {key_expr} AS k, {label_expr} AS label, {sub_expr} AS sub, \
                COUNT(*) AS n, COALESCE(SUM(p.amount_cent), 0) AS amt \
         FROM pay_payment p \
         JOIN ct_contract c ON c.id = p.contract_id \
         LEFT JOIN prj_project pj ON pj.id = c.project_id \
         LEFT JOIN org_contractor o ON o.id = c.contractor_id \
         LEFT JOIN v_contract_summary cs ON cs.contract_id = p.contract_id \
         WHERE p.status = 'PAID' AND p.is_deleted = 0 \
           AND (?1 = '' OR p.pay_date >= ?1) \
           AND (?2 = '' OR p.pay_date <= ?2) \
           AND (?3 = -1 OR (CASE WHEN COALESCE(cs.pay_rate_bp, 0) >= 10000 THEN 1 ELSE 0 END) = ?3) \
           AND (?4 = '' OR c.contract_type = ?4) \
           AND (?5 = '' OR pj.project_name LIKE '%' || ?5 || '%' ESCAPE '\\' \
                         OR o.contractor_name LIKE '%' || ?5 || '%' ESCAPE '\\' \
                         OR c.contract_no LIKE '%' || ?5 || '%' ESCAPE '\\' \
                         OR c.contract_name LIKE '%' || ?5 || '%' ESCAPE '\\') \
         GROUP BY {key_expr} \
         ORDER BY amt DESC, label"
    );
    // 与日志筛选同口径：关键词按字面子串匹配，% / _ 不是模式
    let kw = crate::db::like_escape(f.keyword.trim());
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![
            f.from.trim(),
            f.to.trim(),
            archived,
            f.contract_type.clone().unwrap_or_default(),
            kw
        ],
        |row| {
            Ok(SummaryRow {
                key: row.get(0)?,
                label: row.get(1)?,
                sub: row.get::<_, Option<String>>(2)?,
                payment_count: row.get(3)?,
                amount: money(row.get(4)?),
            })
        },
    )?;
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
    let required = [
        "created_at",
        "created_by",
        "updated_at",
        "updated_by",
        "version",
    ];
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

// ---------------------------------------------------------------- 档案查找
//
// 新建合同的三个输入框允许「输入任意字符 + 联想」，落库前要把文本解析成
// 具体档案：先按精确匹配找，找不到再由 service 层自动建档。

/// 按「项目编码 或 项目名称」精确查找（未删除的）。
pub fn find_project_by_text(conn: &Connection, text: &str) -> Result<Option<Project>> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let sql = "SELECT id, project_code, project_name, remark, is_deleted, version, \
        created_at, created_by, updated_at, updated_by \
        FROM prj_project WHERE is_deleted = 0 AND (project_code = ?1 OR project_name = ?1) \
        ORDER BY project_code LIMIT 1";
    let p = conn
        .query_row(sql, [t], |row| {
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
        })
        .optional()?;
    Ok(p)
}

/// 按单位名称精确查找（未删除的）。甲方与承包单位共用这张表，故不区分角色。
pub fn find_contractor_by_name(conn: &Connection, name: &str) -> Result<Option<Contractor>> {
    let t = name.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let sql =
        "SELECT id, contractor_name, role_type, credit_code, bank_name, bank_account, remark, \
        is_deleted, version, created_at, created_by, updated_at, updated_by \
        FROM org_contractor WHERE is_deleted = 0 AND contractor_name = ?1 LIMIT 1";
    let c = conn
        .query_row(sql, [t], |row| {
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
        })
        .optional()?;
    Ok(c)
}

/// 历史合同里出现过的发包人名称（去重）。
///
/// 甲方改为可联想输入后，老数据的 `party_a` 没有对应档案，
/// 用它兜住「以前录过的甲方这次还得能手打出来」的问题。
pub fn list_party_a_names(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT party_a FROM ct_contract \
         WHERE is_deleted = 0 AND length(trim(party_a)) > 0 ORDER BY party_a",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ---------------------------------------------------------------- 合同附件

const ATTACHMENT_COLS: &str = "id, contract_id, att_type, file_name, mime_type, size_bytes, \
    sha256, remark, created_at, created_by, updated_at, updated_by, version";

fn map_attachment(row: &Row) -> rusqlite::Result<ctp_core::attachment::Attachment> {
    Ok(ctp_core::attachment::Attachment {
        id: row.get(0)?,
        contract_id: row.get(1)?,
        kind_code: row.get(2)?,
        file_name: row.get(3)?,
        mime_type: row.get(4)?,
        size_bytes: row.get(5)?,
        sha256: row.get(6)?,
        remark: row.get(7)?,
        created_at: row.get(8)?,
        created_by: row.get(9)?,
        updated_at: row.get(10)?,
        updated_by: row.get(11)?,
        version: row.get(12)?,
    })
}

/// 某合同的附件元数据（不含字节），按类别顺序返回。
pub fn list_attachments(
    conn: &Connection,
    contract_id: &str,
) -> Result<Vec<ctp_core::attachment::Attachment>> {
    let sql = format!(
        "SELECT {ATTACHMENT_COLS} FROM ct_contract_attachment WHERE contract_id = ?1 \
         ORDER BY CASE att_type WHEN 'BID_NOTICE' THEN 1 WHEN 'CONTRACT' THEN 2 \
         WHEN 'PROGRESS_SETTLEMENT' THEN 3 ELSE 4 END"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([contract_id], map_attachment)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 写入或替换某类附件。`(contract_id, att_type)` 唯一，冲突即覆盖。
pub fn upsert_attachment(
    conn: &Connection,
    a: &ctp_core::attachment::Attachment,
    content: &[u8],
) -> Result<()> {
    let size = content.len() as i64;
    if size <= 0 {
        return Err(StoreError::Inconsistent("附件内容为空".into()));
    }
    if size > ctp_core::attachment::MAX_ATTACHMENT_BYTES {
        return Err(StoreError::Inconsistent(format!(
            "附件超过 {} MB 上限",
            ctp_core::attachment::MAX_ATTACHMENT_BYTES / 1024 / 1024
        )));
    }
    conn.execute(
        "INSERT INTO ct_contract_attachment \
            (id, contract_id, att_type, file_name, mime_type, size_bytes, sha256, content, \
             remark, created_at, created_by, updated_at, updated_by, version) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?10,?11,1) \
         ON CONFLICT(contract_id, att_type) DO UPDATE SET \
            file_name = excluded.file_name, mime_type = excluded.mime_type, \
            size_bytes = excluded.size_bytes, sha256 = excluded.sha256, \
            content = excluded.content, remark = excluded.remark, \
            updated_at = excluded.updated_at, updated_by = excluded.updated_by, \
            version = ct_contract_attachment.version + 1",
        rusqlite::params![
            a.id,
            a.contract_id,
            a.kind_code,
            a.file_name,
            a.mime_type,
            size,
            a.sha256,
            content,
            a.remark,
            a.updated_at,
            a.updated_by
        ],
    )?;
    Ok(())
}

/// 取附件字节（打开 / 另存时用）。调用方需自行控制内存，单文件上限 50 MB。
pub fn attachment_bytes(conn: &Connection, id: &str) -> Result<Vec<u8>> {
    let bytes: Option<Vec<u8>> = conn
        .query_row(
            "SELECT content FROM ct_contract_attachment WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    bytes.ok_or_else(|| StoreError::Inconsistent(format!("附件 {id} 不存在")))
}

/// 删除一条附件；返回是否真的删掉了。
/// 删除附件；返回被删记录的 `(att_type, file_name)`，供审计日志记下对象名称。
///
/// 删完就查不到名字了，所以必须先取再删；没命中返回 `None`（调用方据此不写日志）。
pub fn delete_attachment(conn: &Connection, id: &str) -> Result<Option<(String, String)>> {
    let meta: Option<(String, String)> = conn
        .query_row(
            "SELECT att_type, file_name FROM ct_contract_attachment WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some(meta) = meta else {
        return Ok(None);
    };
    let n = conn.execute("DELETE FROM ct_contract_attachment WHERE id = ?1", [id])?;
    Ok(if n > 0 { Some(meta) } else { None })
}

/// 该合同是否已上传某一类附件。
pub fn has_attachment(conn: &Connection, contract_id: &str, kind_code: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ct_contract_attachment WHERE contract_id = ?1 AND att_type = ?2",
        rusqlite::params![contract_id, kind_code],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 附件数量（详情卡的「附件 n/4」用）。
pub fn attachment_count(conn: &Connection, contract_id: &str) -> Result<i64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ct_contract_attachment WHERE contract_id = ?1",
        [contract_id],
        |r| r.get(0),
    )?;
    Ok(n)
}

// ---------------------------------------------------------------- 变更历史

/// C8：写一条合同变更历史（金额 `AMOUNT` / 审计价 `AUDIT_PRICE`）。
///
/// 与业务更新在**同一个事务**内调用，取代 `sys_meta` 里的全局 JSON 数组——
/// 那两个键跨合同共享 50 条上限且不带合同 id，合同一多审计链就被挤断。
/// `old_cent` / `new_cent` 为 `None` 表示「未审定 → 有值」或「清空回未审定」。
///
/// 用具名结构体而不是一串位置参数：这两个都是 `Option<i64>`，按位置传极易
/// 前后颠倒，而历史表写反了不会报错、只在几个月后对账时才暴露。
pub struct ChangeHistoryEntry<'a> {
    pub contract_id: &'a str,
    pub change_type: &'a str,
    pub old_cent: Option<i64>,
    pub new_cent: Option<i64>,
    pub reason: &'a str,
    pub operator_id: &'a str,
    pub created_at: &'a str,
}

pub fn insert_change_history(conn: &Connection, e: &ChangeHistoryEntry<'_>) -> Result<()> {
    conn.execute(
        "INSERT INTO ct_contract_change_history \
            (id, contract_id, change_type, old_cent, new_cent, reason, operator_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            ctp_core::ids::new_id("CHH"),
            e.contract_id,
            e.change_type,
            e.old_cent,
            e.new_cent,
            e.reason,
            e.operator_id,
            e.created_at
        ],
    )?;
    Ok(())
}

/// 某合同最近一条指定类型变更的原因（弹窗回填用；`change_type` 传
/// `AMOUNT` / `AUDIT_PRICE`）。没有历史时返回 `None`。
pub fn last_change_reason(
    conn: &Connection,
    contract_id: &str,
    change_type: &str,
) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    let v: Option<String> = conn
        .query_row(
            "SELECT reason FROM ct_contract_change_history \
             WHERE contract_id = ?1 AND change_type = ?2 AND reason IS NOT NULL \
             ORDER BY created_at DESC, id DESC LIMIT 1",
            rusqlite::params![contract_id, change_type],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.filter(|s| !s.trim().is_empty()))
}
