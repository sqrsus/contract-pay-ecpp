use crate::history;
use ctp_core::attachment::{Attachment, AttachmentKind};
use ctp_core::contract::{Contract, ContractStatus, ContractType, PricingMethod};
use ctp_core::date::{now_iso, validate_iso_date};
use ctp_core::digest;
use ctp_core::error::DomainError;
use ctp_core::ids;
use ctp_core::money::Money;
use ctp_core::payment::{PayMethod, Payment, PaymentStatus};
use ctp_core::project::{Contractor, ContractorRole, Project};
use ctp_core::quota::PaymentQuota;
use ctp_core::rate::Rate;
use ctp_store::{db, repo, Result, StoreError};
use rusqlite::Connection;

fn year_of(date: &str) -> String {
    date.get(0..4).unwrap_or("0000").to_string()
}

/// 合同登记入参（FR-CT-01）。
///
/// `project_id` / `contractor_id` 与 `*_text` 二选一：
/// 界面用输入框录入时只给文本，由 `create_contract` 负责「先查后建」；
/// 已有调用方（演示数据、测试）传 id，行为不变。
#[derive(Debug, Clone)]
pub struct NewContract {
    pub project_id: String,
    pub contract_name: String,
    pub contract_type: ContractType,
    pub contract_category_code: String,
    pub category_id: Option<String>,
    pub party_a: String,
    pub contractor_id: String,
    /// 项目文本（项目编码或名称）。`project_id` 为空时用来解析/建档。
    pub project_text: Option<String>,
    /// 承包单位名称。`contractor_id` 为空时用来解析/建档。
    pub contractor_text: Option<String>,
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
            project_text: None,
            contractor_text: None,
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

// ---------------------------------------------------------------- 档案解析
//
// 新建合同页把「所属项目 / 承包单位 / 发包人」改成可自由输入的联想框，
// 因此落库前必须把文本解析成档案 id。规则：精确命中就复用，否则自动建档，
// 并写一条审计日志，事后能在档案里查到这个条目是怎么来的。

/// 合同类型 → 参建单位角色（自动建档时按类型映射）。
fn role_for_type(t: ContractType) -> ContractorRole {
    match t {
        ContractType::Construct => ContractorRole::Construct,
        ContractType::Supervise => ContractorRole::Supervise,
        ContractType::Design => ContractorRole::Design,
        ContractType::Survey => ContractorRole::Survey,
        ContractType::Supply => ContractorRole::Supplier,
        ContractType::Service => ContractorRole::Other,
    }
}

/// 取单位 id：名称已在档就复用，否则按给定角色建档。甲方与承包单位共用。
fn ensure_contractor(
    conn: &Connection,
    name: &str,
    role: ContractorRole,
    operator: &str,
) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DomainError::MissingField("单位名称").into());
    }
    if let Some(c) = repo::find_contractor_by_name(conn, name)? {
        return Ok(c.id);
    }
    let now = now_iso();
    let c = Contractor {
        id: ids::new_id("ORG"),
        contractor_name: name.to_string(),
        role_type: role,
        credit_code: None,
        bank_name: None,
        bank_account: None,
        remark: Some("新建合同时自动建档".into()),
        is_deleted: false,
        version: 1,
        created_at: now.clone(),
        created_by: Some(operator.to_string()),
        updated_at: now,
        updated_by: Some(operator.to_string()),
    };
    c.validate()?;
    repo::insert_contractor(conn, &c)?;
    db::insert_audit_log(
        conn,
        "org_contractor",
        &c.id,
        "CREATE",
        Some(&format!("{}（{}）", c.contractor_name, role.label())),
        operator,
    )?;
    Ok(c.id)
}

/// 解析所属项目：`id` 优先；否则按「编码或名称」命中；再不然建档。
fn resolve_project(
    conn: &Connection,
    id: &str,
    text: Option<&str>,
    operator: &str,
) -> Result<String> {
    if !id.trim().is_empty() {
        return Ok(id.trim().to_string());
    }
    let text = text.unwrap_or("").trim();
    if text.is_empty() {
        return Err(DomainError::MissingField("所属项目").into());
    }
    // 输入框展示的是「编码 名称」，先按整串找，再拆成编码 / 名称分别找
    let mut probes = vec![text.to_string()];
    if let Some((head, tail)) = text.split_once(' ') {
        probes.push(head.trim().to_string());
        probes.push(tail.trim().to_string());
    }
    for probe in &probes {
        if let Some(p) = repo::find_project_by_text(conn, probe)? {
            return Ok(p.id);
        }
    }
    let now = now_iso();
    let year = year_of(&now);
    let seq = db::next_sequence(conn, &format!("project_code:{year}"))?;
    let p = Project {
        id: ids::new_id("PRJ"),
        project_code: format!("PRJ-{year}-{seq:03}"),
        project_name: text.to_string(),
        remark: Some("新建合同时自动建档".into()),
        is_deleted: false,
        version: 1,
        created_at: now.clone(),
        created_by: Some(operator.to_string()),
        updated_at: now,
        updated_by: Some(operator.to_string()),
    };
    p.validate()?;
    repo::insert_project(conn, &p)?;
    db::insert_audit_log(
        conn,
        "prj_project",
        &p.id,
        "CREATE",
        Some(&format!("{} {}", p.project_code, p.project_name)),
        operator,
    )?;
    Ok(p.id)
}

fn resolve_contractor(
    conn: &Connection,
    id: &str,
    text: Option<&str>,
    ctype: ContractType,
    operator: &str,
) -> Result<String> {
    if !id.trim().is_empty() {
        return Ok(id.trim().to_string());
    }
    let text = text.unwrap_or("").trim();
    if text.is_empty() {
        return Err(DomainError::MissingField("承包单位").into());
    }
    ensure_contractor(conn, text, role_for_type(ctype), operator)
}

/// 发包人：并入单位库，角色固定为业主。
fn resolve_owner(conn: &Connection, name: &str, operator: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DomainError::MissingField("发包人（甲方）").into());
    }
    ensure_contractor(conn, name, ContractorRole::Owner, operator)
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

    // 三个输入框的文本 → 具体档案；找不到就自动建档（甲方记为业主角色）
    let project_id = resolve_project(&tx, &input.project_id, input.project_text.as_deref(), operator)?;
    let contractor_id = resolve_contractor(
        &tx,
        &input.contractor_id,
        input.contractor_text.as_deref(),
        input.contract_type,
        operator,
    )?;
    let party_a = resolve_owner(&tx, &input.party_a, operator)?;

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
        project_id,
        contract_no: contract_no.clone(),
        contract_name: input.contract_name,
        contract_type: input.contract_type,
        contract_category_code: input.contract_category_code,
        category_id: input.category_id,
        party_a,
        contractor_id,
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
///
/// 生效是「合同正本必须齐备」的卡点：存草稿阶段允许缺附件，避免手里没有
/// 电子件时连合同信息都录不进去；一旦要进入可拨款状态，就要求合同附件在位。
pub fn activate_contract(conn: &mut Connection, contract_id: &str, operator: &str) -> Result<Contract> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut c = repo::get_contract(&tx, contract_id)?;
    if !repo::has_attachment(&tx, &c.id, AttachmentKind::Contract.code())? {
        return Err(DomainError::AttachmentRequired(AttachmentKind::Contract.label()).into());
    }
    c.transit(ContractStatus::Active)?;
    c.updated_at = now_iso();
    c.updated_by = Some(operator.to_string());
    repo::update_contract(&tx, &c)?;
    db::insert_audit_log(&tx, "ct_contract", &c.id, "ACTIVATE", None, operator)?;
    tx.commit()?;
    Ok(c)
}

// ---------------------------------------------------------------- 合同附件

/// 保存或替换某类附件（每类仅一份，冲突即覆盖）。
///
/// 文件本体写入 `ct_contract_attachment.content`，因此这里挡住两件事：
/// 空文件与超过 50 MB 的文件——它们是数据库体积失控的主要来源。
pub fn save_attachment(
    conn: &Connection,
    contract_id: &str,
    kind: AttachmentKind,
    file_name: &str,
    mime_type: Option<&str>,
    content: &[u8],
    operator: &str,
) -> Result<Attachment> {
    if content.is_empty() {
        return Err(StoreError::Inconsistent(format!(
            "「{}」文件内容为空",
            kind.label()
        )));
    }
    let max = ctp_core::attachment::MAX_ATTACHMENT_BYTES;
    if content.len() as i64 > max {
        return Err(StoreError::Inconsistent(format!(
            "「{}」{:.1} MB，超过 {} MB 上限",
            kind.label(),
            content.len() as f64 / (1024.0 * 1024.0),
            max / 1024 / 1024
        )));
    }
    let name = file_name.trim();
    if name.is_empty() {
        return Err(DomainError::MissingField("附件文件名").into());
    }
    let now = now_iso();
    let a = Attachment {
        id: ids::new_id("ATT"),
        contract_id: contract_id.to_string(),
        kind_code: kind.code().to_string(),
        file_name: name.to_string(),
        mime_type: mime_type.map(|s| s.to_string()),
        size_bytes: content.len() as i64,
        sha256: digest::sha256_hex(content),
        remark: None,
        created_at: now.clone(),
        created_by: Some(operator.to_string()),
        updated_at: now,
        updated_by: Some(operator.to_string()),
        version: 1,
    };
    repo::upsert_attachment(conn, &a, content)?;
    db::insert_audit_log(
        conn,
        "ct_contract_attachment",
        &a.id,
        "UPSERT",
        Some(&format!("{} · {}", kind.label(), name)),
        operator,
    )?;
    // 覆盖已有行时主键仍是旧 id，回读一次拿真实记录
    let saved = repo::list_attachments(conn, contract_id)?
        .into_iter()
        .find(|x| x.kind_code == kind.code());
    Ok(saved.unwrap_or(a))
}

pub fn list_attachments(conn: &Connection, contract_id: &str) -> Result<Vec<Attachment>> {
    repo::list_attachments(conn, contract_id)
}

pub fn attachment_bytes(conn: &Connection, id: &str) -> Result<Vec<u8>> {
    repo::attachment_bytes(conn, id)
}

pub fn delete_attachment(conn: &Connection, id: &str, operator: &str) -> Result<()> {
    if repo::delete_attachment(conn, id)? {
        db::insert_audit_log(conn, "ct_contract_attachment", id, "DELETE", None, operator)?;
    }
    Ok(())
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

// ---------------------------------------------------------------- 报表导出

/// 从视图行构造报表结果（屏幕表格与导出共用这一份）。
pub fn report_of(conn: &Connection, kind: crate::report::ReportKind) -> Result<crate::report::ReportResult> {
    use crate::report::{ReportKind, ReportResult};
    Ok(match kind {
        ReportKind::CategorySummary => ReportResult::category(&repo::category_summary(conn)?),
        ReportKind::PeriodSummary => ReportResult::period(&repo::period_summary(conn)?),
    })
}

/// 导出一张报表到 .xlsx，并写一条 `EXPORT` 审计日志（FR-SYS-07）。
///
/// 只用 `&Connection`：导出不改业务数据，审计日志是普通 INSERT，不需要 `BEGIN IMMEDIATE`。
pub fn export_report(
    conn: &Connection,
    kind: crate::report::ReportKind,
    path: &std::path::Path,
    operator: &str,
) -> Result<(usize, usize)> {
    let result = report_of(conn, kind)?;
    let (rows, cols) = crate::xlsx::write_report(&result, path, operator)?;
    let detail = serde_json::json!({
        "report": kind.code(),
        "file": path.to_string_lossy(),
        "rows": rows,
        "columns": cols,
    })
    .to_string();
    db::insert_audit_log(conn, "report", kind.code(), "EXPORT", Some(&detail), operator)?;
    Ok((rows, cols))
}

// ---------------------------------------------------------------- 付款打印数据

/// 选中一张拨款单 → 解析 9 个合并域的取值（本轮只供界面预览，不产出 docx）。
pub fn payment_print_data(
    conn: &Connection,
    payment_id: &str,
) -> std::result::Result<crate::print::PaymentPrintData, crate::print::PrintError> {
    crate::print::resolve(conn, payment_id)
}
