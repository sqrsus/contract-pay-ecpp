use crate::history;
use ctp_core::contract::{Contract, ContractStatus, ContractType, PricingMethod};
use ctp_core::date::{now_iso, validate_iso_date};
use ctp_core::error::DomainError;
use ctp_core::ids;
use ctp_core::money::Money;
use ctp_core::payment::{PayMethod, Payment, PaymentStatus};
use ctp_core::quota::PaymentQuota;
use ctp_core::rate::Rate;
use ctp_store::{db, repo, Result, StoreError};
use rusqlite::Connection;

fn year_of(date: &str) -> String {
    date.get(0..4).unwrap_or("0000").to_string()
}

/// 合同登记入参（FR-CT-01）。
#[derive(Debug, Clone)]
pub struct NewContract {
    pub project_id: String,
    pub contract_name: String,
    pub contract_type: ContractType,
    pub contract_category_code: String,
    pub category_id: Option<String>,
    pub party_a: String,
    pub contractor_id: String,
    pub building_id: Option<String>,
    pub sign_date: String,
    pub original_amount: Money,
    pub duration_days: i64,
    pub planned_start: Option<String>,
    pub planned_end: Option<String>,
    pub pricing_method: Option<PricingMethod>,
    pub retention_rate_bp: i32,
    pub advance_rate_bp: i32,
    pub payment_terms: String,
    pub remark: Option<String>,
}

impl Default for NewContract {
    fn default() -> Self {
        NewContract {
            project_id: String::new(),
            contract_name: String::new(),
            contract_type: ContractType::Construct,
            contract_category_code: "GC".into(),
            category_id: None,
            party_a: String::new(),
            contractor_id: String::new(),
            building_id: None,
            sign_date: ctp_core::date::today_iso(),
            original_amount: Money::ZERO,
            duration_days: 0,
            planned_start: None,
            planned_end: None,
            pricing_method: None,
            retention_rate_bp: 300,
            advance_rate_bp: 0,
            payment_terms: String::new(),
            remark: None,
        }
    }
}

/// 拨款单登记入参（FR-PAY-06，单级登记、无申请单）。
#[derive(Debug, Clone)]
pub struct NewPayment {
    pub contract_id: String,
    pub amount: Money,
    pub pay_date: String,
    pub reason: Option<String>,
    pub pay_method: PayMethod,
    pub title: Option<String>,
    pub bank_serial_no: Option<String>,
    pub pay_account: Option<String>,
    pub receive_account: Option<String>,
    pub remark: Option<String>,
}

impl NewPayment {
    pub fn new(contract_id: impl Into<String>, amount: Money, pay_date: impl Into<String>) -> Self {
        NewPayment {
            contract_id: contract_id.into(),
            amount,
            pay_date: pay_date.into(),
            reason: None,
            pay_method: PayMethod::Transfer,
            title: None,
            bank_serial_no: None,
            pay_account: None,
            receive_account: None,
            remark: None,
        }
    }
}

/// 视图比率与应用层比率的不一致项（H1 断言输出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateMismatch {
    pub contract_no: String,
    pub view_bp: i32,
    pub app_bp: i32,
}

// ---------------------------------------------------------------- 合同

pub fn create_contract(
    conn: &mut Connection,
    input: NewContract,
    operator: &str,
) -> Result<Contract> {
    validate_iso_date(&input.sign_date)?;
    if input.payment_terms.trim().is_empty() {
        return Err(DomainError::MissingField("付款条款").into());
    }
    if input.contract_category_code.trim().is_empty() {
        return Err(DomainError::MissingField("合同类别").into());
    }

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let now = now_iso();
    let year = year_of(&input.sign_date);
    let seq = db::next_sequence(&tx, &format!("contract_no:{year}"))?;
    let contract_no = ids::contract_no(&year, seq);

    if repo::find_contract_by_no(&tx, &contract_no)?.is_some() {
        return Err(StoreError::Inconsistent(format!(
            "合同编号 {contract_no} 已存在（编号唯一约束）"
        )));
    }
    if let Some(cat) = &input.category_id {
        if !repo::category_exists(&tx, cat)? {
            return Err(StoreError::DictMissing(format!("会计分类 {cat}")));
        }
    }

    let c = Contract {
        id: ids::new_id("CT"),
        project_id: input.project_id,
        contract_no: contract_no.clone(),
        contract_name: input.contract_name,
        contract_type: input.contract_type,
        contract_category_code: input.contract_category_code,
        category_id: input.category_id,
        party_a: input.party_a,
        contractor_id: input.contractor_id,
        building_id: input.building_id,
        sort_no: seq,
        sign_date: input.sign_date,
        original_amount_cent: input.original_amount,
        audit_price_cent: None,
        audit_date: None,
        audit_org: None,
        audit_report_no: None,
        duration_days: input.duration_days,
        planned_start: input.planned_start,
        planned_end: input.planned_end,
        actual_start: None,
        actual_end: None,
        pricing_method: input.pricing_method,
        retention_rate_bp: input.retention_rate_bp,
        advance_rate_bp: input.advance_rate_bp,
        status: ContractStatus::Draft,
        payment_terms: input.payment_terms,
        remark: input.remark,
        is_deleted: false,
        version: 1,
        created_at: now.clone(),
        created_by: Some(operator.to_string()),
        updated_at: now,
        updated_by: Some(operator.to_string()),
    };
    c.validate()?;
    repo::insert_contract(&tx, &c)?;
    db::insert_audit_log(
        &tx,
        "ct_contract",
        &c.id,
        "CREATE",
        Some(&format!("{} {}", c.contract_no, c.contract_name)),
        operator,
    )?;
    tx.commit()?;
    Ok(c)
}

/// 合同生效（草稿 → 生效）。本期无审批环节，由登记人直接生效。
pub fn activate_contract(conn: &mut Connection, contract_id: &str, operator: &str) -> Result<Contract> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut c = repo::get_contract(&tx, contract_id)?;
    c.transit(ContractStatus::Active)?;
    c.updated_at = now_iso();
    c.updated_by = Some(operator.to_string());
    repo::update_contract(&tx, &c)?;
    db::insert_audit_log(&tx, "ct_contract", &c.id, "ACTIVATE", None, operator)?;
    tx.commit()?;
    Ok(c)
}

/// 合同作废（仅限无任何已付记录的合同，4.2.3）。
pub fn void_contract(conn: &mut Connection, contract_id: &str, operator: &str) -> Result<Contract> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut c = repo::get_contract(&tx, contract_id)?;
    if repo::paid_payment_count(&tx, &c.id)? > 0 {
        return Err(DomainError::ContractHasPayments.into());
    }
    c.transit(ContractStatus::Void)?;
    c.updated_at = now_iso();
    c.updated_by = Some(operator.to_string());
    repo::update_contract(&tx, &c)?;
    db::insert_audit_log(&tx, "ct_contract", &c.id, "VOID", None, operator)?;
    tx.commit()?;
    Ok(c)
}

/// 合同金额（暂定）编辑（BR-22）：必填原因、留痕、已审定合同不改基准。
pub fn edit_contract_amount(
    conn: &mut Connection,
    contract_id: &str,
    new_amount: Money,
    reason: &str,
    operator: &str,
) -> Result<Contract> {
    if reason.trim().is_empty() {
        return Err(DomainError::MissingField("金额编辑原因").into());
    }
    if new_amount.cent() < 0 {
        return Err(DomainError::NegativeAmount(new_amount.cent()).into());
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut c = repo::get_contract(&tx, contract_id)?;
    if c.status == ContractStatus::Void {
        return Err(DomainError::IllegalState("作废合同不可编辑金额".into()).into());
    }
    let old = c.original_amount_cent;
    if old == new_amount {
        return Err(DomainError::Invariant("新旧金额相同，无需留痕".into()).into());
    }
    let at = now_iso();
    c.original_amount_cent = new_amount;
    c.updated_at = at.clone();
    c.updated_by = Some(operator.to_string());
    repo::update_contract(&tx, &c)?;

    history::append(
        &tx,
        history::KEY_CONTRACT_AMOUNT,
        history::amount_entry(&at, old.cent(), new_amount.cent(), reason, operator),
        operator,
    )?;
    db::insert_audit_log(
        &tx,
        "ct_contract",
        &c.id,
        "EDIT_AMOUNT",
        Some(&format!("{} -> {}：{reason}", old.format_yuan(), new_amount.format_yuan())),
        operator,
    )?;
    tx.commit()?;
    Ok(c)
}

/// 审计价登记 / 修改 / 清空（BR-14 / BR-16）。
///
/// * `price` 为 `None` 或 `<= 0` 即「未审定」，基准回退合同金额；
/// * 必填原因，写 `audit_price_history`；
/// * 只改档案与基准，**不改写任何历史拨款流水**（累计已付保持不变）。
pub fn register_audit_price(
    conn: &mut Connection,
    contract_id: &str,
    price: Option<Money>,
    audit_date: Option<String>,
    audit_org: Option<String>,
    audit_report_no: Option<String>,
    reason: &str,
    operator: &str,
) -> Result<Contract> {
    if reason.trim().is_empty() {
        return Err(DomainError::MissingField("审计价变更原因").into());
    }
    if let Some(d) = &audit_date {
        if !d.trim().is_empty() {
            validate_iso_date(d)?;
        }
    }
    if let Some(p) = price {
        if p.cent() < 0 {
            return Err(DomainError::NegativeAmount(p.cent()).into());
        }
    }

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut c = repo::get_contract(&tx, contract_id)?;
    let old = c.audit_price().map(|m| m.cent());
    // 归一：<= 0 一律按未审定处理（4.2.7）
    let normalized = price.filter(|p| p.cent() > 0);
    c.audit_price_cent = normalized;
    c.audit_date = audit_date.filter(|d| !d.trim().is_empty());
    c.audit_org = audit_org.filter(|d| !d.trim().is_empty());
    c.audit_report_no = audit_report_no.filter(|d| !d.trim().is_empty());
    c.updated_at = now_iso();
    c.updated_by = Some(operator.to_string());
    repo::update_contract(&tx, &c)?;

    history::append(
        &tx,
        history::KEY_AUDIT_PRICE,
        history::audit_entry(
            &c.updated_at,
            old,
            normalized.map(|m| m.cent()),
            reason,
            operator,
        ),
        operator,
    )?;
    let detail = match normalized {
        Some(p) => format!(
            "审计价 {}（原 {}）：{reason}",
            p.format_yuan(),
            old.map(|v| Money::from_cent(v).format_yuan())
                .unwrap_or_else(|| "未审定".into())
        ),
        None => format!("清空审计价，基准回退合同金额：{reason}"),
    };
    db::insert_audit_log(&tx, "ct_contract", &c.id, "SET_AUDIT_PRICE", Some(&detail), operator)?;
    tx.commit()?;
    Ok(c)
}

// ---------------------------------------------------------------- 拨款

/// 登记拨款单：额度校验（BR-01）与流水号自增在同一事务内。
pub fn register_payment(
    conn: &mut Connection,
    input: NewPayment,
    operator: &str,
) -> Result<Payment> {
    if input.amount.cent() <= 0 {
        return Err(DomainError::NonPositivePayment(input.amount.cent()).into());
    }
    validate_iso_date(&input.pay_date)?;

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut c = repo::get_contract(&tx, &input.contract_id)?;
    let paid = repo::paid_net(&tx, &c.id)?;
    let tolerance = db::tolerance(&tx)?;
    // 1) 合同须可拨款；2) 本次金额不得超过可付额度
    let quota = PaymentQuota::for_new_payment(&c, paid, tolerance)?;
    quota.check_new_payment(input.amount)?;

    let now = now_iso();
    let year = year_of(&input.pay_date);
    let seq = db::next_sequence(&tx, &format!("payment_no:{year}"))?;
    let payment_no = ids::payment_no(&year, seq);

    let p = Payment {
        id: ids::new_id("PM"),
        payment_no,
        title: input.title,
        sort_no: seq,
        period_no: seq,
        contract_id: c.id.clone(),
        project_id: c.project_id.clone(),
        pay_date: input.pay_date,
        amount_cent: input.amount,
        reason: input.reason,
        pay_method: input.pay_method,
        bank_serial_no: input.bank_serial_no,
        pay_account: input.pay_account,
        receive_account: input.receive_account,
        status: PaymentStatus::Paid,
        operator_id: operator.to_string(),
        print_count: 0,
        remark: input.remark,
        is_deleted: false,
        version: 1,
        created_at: now.clone(),
        created_by: Some(operator.to_string()),
        updated_at: now,
        updated_by: Some(operator.to_string()),
    };
    p.validate()?;
    repo::insert_payment(&tx, &p)?;

    // 首笔拨款：生效 → 履行中（4.2.3）
    if c.status == ContractStatus::Active {
        c.transit(ContractStatus::Performing)?;
        c.updated_at = p.created_at.clone();
        c.updated_by = Some(operator.to_string());
        repo::update_contract(&tx, &c)?;
    }

    db::insert_audit_log(
        &tx,
        "pay_payment",
        &p.id,
        "REGISTER",
        Some(&format!(
            "{} {} 元（合同 {}，已付 {} / 基准 {}）",
            p.payment_no,
            p.amount_cent.format_yuan(),
            c.contract_no,
            (paid + p.amount_cent).format_yuan(),
            quota.base_amount.format_yuan()
        )),
        operator,
    )?;
    tx.commit()?;
    Ok(p)
}

/// 拨款单作废（评审 H5：以「作废 + 重登」替代红冲）。作废后不再计入已付净额。
pub fn void_payment(
    conn: &mut Connection,
    payment_id: &str,
    reason: &str,
    operator: &str,
) -> Result<Payment> {
    if reason.trim().is_empty() {
        return Err(DomainError::MissingField("作废原因").into());
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut p = repo::get_payment(&tx, payment_id)?;
    p.void()?;
    p.updated_at = now_iso();
    p.updated_by = Some(operator.to_string());
    repo::update_payment(&tx, &p)?;
    db::insert_audit_log(
        &tx,
        "pay_payment",
        &p.id,
        "VOID",
        Some(&format!("作废 {}：{reason}", p.payment_no)),
        operator,
    )?;
    tx.commit()?;
    Ok(p)
}

// ---------------------------------------------------------------- 查询与对账

/// 当前可付额度（展示用，不在事务内加锁）。
pub fn quota_of(conn: &Connection, contract_id: &str) -> Result<PaymentQuota> {
    let c = repo::get_contract(conn, contract_id)?;
    let paid = repo::paid_net(conn, contract_id)?;
    let tolerance = db::tolerance(conn)?;
    Ok(PaymentQuota::compute(&c, paid, Money::ZERO, tolerance)?)
}

/// 口径对账（H1 / 5.5 约束第 5 条）：视图 `pay_rate_bp` 必须等于 `Rate::ratio`。
/// 返回空集合即「差异为 0」。
pub fn reconcile(conn: &Connection) -> Result<Vec<RateMismatch>> {
    let mut out = Vec::new();
    for (contract_no, view_bp, app_bp) in repo::summary_with_expected_rate(conn)? {
        if view_bp != app_bp {
            out.push(RateMismatch {
                contract_no,
                view_bp,
                app_bp,
            });
        }
    }
    Ok(out)
}

/// 容差率读写（`sys_meta.tol_bp`）。
pub fn set_tolerance(conn: &mut Connection, bp: Rate, operator: &str) -> Result<()> {
    if !(0..=1_000).contains(&bp.bp()) {
        return Err(DomainError::Invariant(format!("容差率越界：{} bp（上限 1000）", bp.bp())).into());
    }
    db::set_meta(conn, "tol_bp", &bp.bp().to_string(), operator)?;
    Ok(())
}
