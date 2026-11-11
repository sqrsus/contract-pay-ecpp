use ctp_core::attachment::{Attachment, AttachmentKind};
use ctp_core::contract::{BidMethod, Contract, ContractStatus, ContractType, PricingMethod};
use ctp_core::date::{now_iso, today_local_iso, validate_iso_date};
use ctp_core::dict;
use ctp_core::digest;
use ctp_core::error::DomainError;
use ctp_core::ids;
use ctp_core::money::Money;
use ctp_core::payment::{PayMethod, Payment, PaymentStatus};
use ctp_core::project::{Contractor, ContractorRole, Project};
use ctp_core::quota::PaymentQuota;
use ctp_core::rate::Rate;
use ctp_store::{db, repo, Result, StoreError};
use rusqlite::{Connection, OptionalExtension};

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
    /// 交工日期（YYYY-MM-DD）
    pub delivery_date: Option<String>,
    pub pricing_method: Option<PricingMethod>,
    pub retention_rate_bp: i32,
    pub advance_rate_bp: i32,
    pub payment_terms: String,
    pub remark: Option<String>,
    // —— 新建合同页扩展字段（评审后增补，全部选填）——
    /// 招标方式
    pub bid_method: Option<BidMethod>,
    /// 开标时间（YYYY-MM-DD）
    pub open_bid_date: Option<String>,
    /// 项目负责人
    pub project_leader: Option<String>,
    /// 联系方式
    pub contact_info: Option<String>,
    /// 招标代理名称；为空时用来解析/建档进 `org_contractor`（角色 AGENT）
    pub bid_agent_text: Option<String>,
}

impl Default for NewContract {
    fn default() -> Self {
        NewContract {
            project_id: String::new(),
            contract_name: String::new(),
            contract_type: ContractType::Engineering,
            contract_category_code: "GC".into(),
            category_id: None,
            party_a: String::new(),
            contractor_id: String::new(),
            project_text: None,
            contractor_text: None,
            building_id: None,
            sign_date: today_local_iso(),
            original_amount: Money::ZERO,
            duration_days: 0,
            planned_start: None,
            planned_end: None,
            delivery_date: None,
            pricing_method: None,
            retention_rate_bp: 300,
            advance_rate_bp: 0,
            payment_terms: String::new(),
            remark: None,
            bid_method: None,
            open_bid_date: None,
            project_leader: None,
            contact_info: None,
            bid_agent_text: None,
        }
    }
}

/// 合同信息修改入参（R-50）。
///
/// 只覆盖**主档字段**。以下字段刻意不在其中，各自有专门的留痕入口：
/// * 合同编号——唯一索引 + 已出报表/纸质单据的引用，改动会让历史对不上；
/// * 合同金额——走「编辑金额」（必填原因 + 写入金额变更历史）；
/// * 审计价——走「登记审计价」（写审计价历史）；
/// * 状态——走「设为生效 / 作废」；
/// * 交工日期——走「项目交工」。
#[derive(Debug, Clone)]
pub struct EditContract {
    pub contract_name: String,
    /// 项目文本（编码或名称），空则拒绝
    pub project_text: String,
    /// 承包单位名称，空则拒绝
    pub contractor_text: String,
    pub party_a: String,
    pub contract_type: ContractType,
    pub contract_category_code: String,
    pub sign_date: String,
    pub duration_days: i64,
    pub planned_start: Option<String>,
    pub planned_end: Option<String>,
    pub pricing_method: Option<PricingMethod>,
    pub retention_rate_bp: i32,
    pub payment_terms: String,
    pub remark: Option<String>,
    pub bid_method: Option<BidMethod>,
    pub open_bid_date: Option<String>,
    pub project_leader: Option<String>,
    pub contact_info: Option<String>,
    /// 招标代理名称：空表示清空代理关系
    pub bid_agent_text: Option<String>,
    /// 合同类型变更的二次确认（R-68）。
    ///
    /// 类型决定付清条件与 97% 封顶规则，静默改类型可能让历史付款瞬间「超付」。
    /// 首次提交传 `false`，服务层返回 `NeedConfirm`；界面弹框确认后带 `true` 重交。
    pub confirm_type_change: bool,
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
    /// 批量导入专用幂等键：同一行内容重复导入时不再重复入账。
    /// 手工登记恒为 `None`；由 `register_payment` 在同一事务内写入 `pay_payment.import_key`。
    pub import_key: Option<String>,
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
            import_key: None,
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
///
/// R-57：合同类型只剩工程/货物/服务三类，但「参建单位角色」是**独立字典**
/// （施工/监理/设计/勘察/供应/业主/其他/代理），不随合同类型一起收窄。
fn role_for_type(t: ContractType) -> ContractorRole {
    match t {
        ContractType::Engineering => ContractorRole::Construct,
        ContractType::Supply => ContractorRole::Supplier,
        ContractType::Service => ContractorRole::Other,
    }
}

/// 把「编码 名称」按首个空白切成两段；任一段为空即视为「没拆开」。
/// 项目名与单位名都可能自带空格，所以调用方还要判断 head 是否真像编码。
///
/// `pub(crate)`：批量合同导入的查重键要按同一套规则归一，不能各写一份。
pub(crate) fn split_code_name(text: &str) -> Option<(&str, &str)> {
    text.find(char::is_whitespace)
        .map(|i| (text[..i].trim(), text[i..].trim()))
        .filter(|(head, tail)| !head.is_empty() && !tail.is_empty())
}

/// 是否像编码前缀：全 ASCII 字母数字与 `- _ .`，长度不超过 32。
/// 用来区分「ORG-001 某单位」（前缀是编码）与「中铁 十二局」（前缀属于名称）。
pub(crate) fn looks_like_code(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 取单位 id：名称已在档就复用，否则按给定角色建档。甲方与承包单位共用。
///
/// 文本口径与所属项目同源（R-52 / P6）：整串先查；若写成「编码 名称」且前缀
/// 确实像编码，则按名称段查 / 建档，不再把「ORG-001 某单位」整体建成一条新档。
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
    if let Some((head, tail)) = split_code_name(name) {
        if looks_like_code(head) {
            if let Some(c) = repo::find_contractor_by_name(conn, tail)? {
                return Ok(c.id);
            }
            return create_contractor(conn, tail, role, operator);
        }
    }
    create_contractor(conn, name, role, operator)
}

/// 按名称新建单位档案并写审计（调用方负责先查重）。
fn create_contractor(
    conn: &Connection,
    name: &str,
    role: ContractorRole,
    operator: &str,
) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DomainError::MissingField("单位名称").into());
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
        Some(&c.contractor_name),
        "CREATE",
        Some(&format!("{}（{}）", c.contractor_name, role.label())),
        operator,
    )?;
    Ok(c.id)
}

/// 解析所属项目：`id` 优先；否则按名称 / 编码命中；再不然建档。
///
/// 匹配顺序是**名称优先**（R-51）：
/// 1. 整串（用户可能只写了编码，也可能只写了名称）；
/// 2. 「编码 名称」里空格后的名称段——旧编码不能劫持用户改过的名称；
/// 3. 空格前的编码，且该项目的名称必须与名称段一致（兼容名称本身带空格）；
/// 4. 编码命中但名称被改过：按名称段新建档案，不静默回退到旧项目；
/// 5. 全都没有：按整串新建档案。
///
/// 旧实现按「整串 → 编码 → 名称」探测，用户把「XAJT 旧项目」改成
/// 「XAJT 新项目」时编码先命中、名称被丢掉，界面上表现为「改了个寂寞」，
/// 保存时报「没有修改任何内容」。
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
    if let Some(p) = repo::find_project_by_text(conn, text)? {
        return Ok(p.id);
    }
    // 只有出现空白时才拆「编码 / 名称」两段；名称本身可能带空格，故按首个空白切
    if let Some((head, tail)) = split_code_name(text) {
        // 名称段优先：用户改的一定是名称，编码只是沿用下来的前缀
        if let Some(p) = repo::find_project_by_text(conn, tail)? {
            return Ok(p.id);
        }
        // 名称段没命中，再看编码；但要求名称段与该档案完全一致，
        // 否则说明用户改过名称，不能按编码认领（就是这里曾被旧编码劫持）
        if let Some(p) = repo::find_project_by_text(conn, head)? {
            if p.project_name == tail {
                return Ok(p.id);
            }
            return create_project(conn, tail, operator);
        }
    }
    create_project(conn, text, operator)
}

/// 按名称新建项目档案（自动编码）并写审计。
fn create_project(conn: &Connection, name: &str, operator: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DomainError::MissingField("所属项目").into());
    }
    let now = now_iso();
    let year = year_of(&now);
    let seq = db::next_sequence(conn, &format!("project_code:{year}"))?;
    let p = Project {
        id: ids::new_id("PRJ"),
        project_code: format!("PRJ-{year}-{seq:03}"),
        project_name: name.to_string(),
        remark: Some("按合同表单录入的项目名称自动建档".into()),
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
        Some(&format!("{} {}", p.project_code, p.project_name)),
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

/// 发包人：并入单位库，角色固定为业主；合同 `party_a` 存名称，不存档案 id。
///
/// `ct_contract.party_a` 是名称列（无外键），详情弹窗直接渲染该值；自动建档
/// 只作为单位档案与联想候选的副作用，返回值必须是用户填写的名称。
fn resolve_owner(conn: &Connection, name: &str, operator: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DomainError::MissingField("发包人（甲方）").into());
    }
    ensure_contractor(conn, name, ContractorRole::Owner, operator)?;
    Ok(name.to_string())
}

// ---------------------------------------------------------------- 合同

/// 「新建合同 + 若干附件 + 可选立即生效」组合写入的附件入参（R-70）。
#[derive(Debug, Clone)]
pub struct AttachmentInput {
    pub kind: AttachmentKind,
    pub file_name: String,
    pub mime_type: Option<String>,
    pub content: Vec<u8>,
}

/// 关联档案存在性校验：只用内部白名单表名拼 SQL，避免动态表名注入。
///
/// 原先 `project_id` 直传、`building_id` / `bid_agent_id` 都不校验，
/// 要等 SQLite 抛外键约束；这里提前给出可读的业务错误。
fn require_ref(conn: &Connection, table: &str, id: &str, label: &str) -> Result<()> {
    let sql = match table {
        "prj_project" => "SELECT COUNT(*) FROM prj_project WHERE id = ?1",
        "org_contractor" => "SELECT COUNT(*) FROM org_contractor WHERE id = ?1",
        "prj_building" => "SELECT COUNT(*) FROM prj_building WHERE id = ?1",
        _ => return Ok(()),
    };
    let n: i64 = conn.query_row(sql, [id], |r| r.get(0))?;
    if n == 0 {
        return Err(StoreError::Inconsistent(format!(
            "{label}「{id}」在档案里不存在，请重新选择"
        )));
    }
    Ok(())
}

/// 「新建合同 + 若干附件 + 可选立即生效」在**同一个 IMMEDIATE 事务**里完成。
///
/// R-70（P1）：此前 GUI 在一个写门面闭包里依次调用三个各自开事务并提交的服务，
/// 第 2 个附件失败时合同、自动建档的项目/单位、第 1 个附件与 CREATE 日志都已落库，
/// 界面却报「新建合同失败」，留下无法回滚的半截数据。合并后任何一步失败整体回滚；
/// 成功路径产生的数据与原先逐条提交完全一致。
pub fn create_contract_bundle(
    conn: &mut Connection,
    input: NewContract,
    attachments: &[AttachmentInput],
    activate: bool,
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
    let c = create_contract_in_tx(&tx, input, operator)?;
    for a in attachments {
        // 与单条上传同一套文件名净化口径（R-69），避免路径 / 保留字符落库
        let safe_name = sanitize_file_name(&a.file_name);
        let _ = save_attachment_in_tx(
            &tx,
            &c.id,
            a.kind,
            &safe_name,
            a.mime_type.as_deref(),
            &a.content,
            operator,
        )?;
    }
    if activate {
        activate_in_tx(&tx, &c.id, operator)?;
    }
    tx.commit()?;
    Ok(c)
}

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
    let c = create_contract_in_tx(&tx, input, operator)?;
    tx.commit()?;
    Ok(c)
}

/// 事务内的合同建档主体；单建与「合同 + 附件 + 生效」组合共用同一段实现。
fn create_contract_in_tx(tx: &Connection, input: NewContract, operator: &str) -> Result<Contract> {
    let now = now_iso();

    // 三个输入框的文本 → 具体档案；找不到就自动建档（甲方记为业主角色）
    let project_id = resolve_project(
        tx,
        &input.project_id,
        input.project_text.as_deref(),
        operator,
    )?;
    let contractor_id = resolve_contractor(
        tx,
        &input.contractor_id,
        input.contractor_text.as_deref(),
        input.contract_type,
        operator,
    )?;
    let party_a = resolve_owner(tx, &input.party_a, operator)?;

    // 招标代理：与承包单位同路径，但以 AGENT 角色建档，避免混入承包单位联想列表
    let bid_agent_id = match input.bid_agent_text.as_deref().map(|s| s.trim()) {
        Some(name) if !name.is_empty() => Some(ensure_contractor(
            tx,
            name,
            ContractorRole::Agent,
            operator,
        )?),
        _ => None,
    };

    // 开标时间仅作格式校验（选填）；空串归一为 None
    let open_bid_date = input
        .open_bid_date
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            validate_iso_date(s)?;
            Ok::<_, StoreError>(s.to_string())
        })
        .transpose()?;

    let year = year_of(&input.sign_date);
    // R-68（C7）：取号撞号时跳过重取，而不是报错回滚。
    // 取号在事务内自增，事务因别的原因回滚后序号不前进；若库里已有同号合同
    // （备份恢复、历史手工补录），直接报错会让「下一个号」永远发不出去。
    // 启动时的 `calibrate_sequences` 已按库内最大编号校准过序号表，
    // 这里再兜一层：连续撞号 5 次仍失败才认定为数据异常。
    let (seq, contract_no) = {
        let mut got = None;
        for _ in 0..5 {
            let s = db::next_sequence(tx, &format!("contract_no:{year}"))?;
            let no = ids::contract_no(&year, s);
            if repo::find_contract_by_no(tx, &no)?.is_none() {
                got = Some((s, no));
                break;
            }
        }
        got.ok_or_else(|| {
            StoreError::Inconsistent(format!(
                "合同编号连续 5 次撞号（{year} 年度），请检查 ct_contract 与 sys_sequence 是否一致"
            ))
        })?
    };
    if let Some(cat) = &input.category_id {
        if !repo::category_exists(tx, cat)? {
            return Err(StoreError::DictMissing(format!("会计分类 {cat}")));
        }
    }

    // R-70：关联档案显式校验，避免「id 直传一个不存在的档案」落到外键约束报错
    require_ref(tx, "prj_project", &project_id, "所属项目")?;
    require_ref(tx, "org_contractor", &contractor_id, "承包单位")?;
    if let Some(id) = bid_agent_id.as_deref() {
        require_ref(tx, "org_contractor", id, "招标代理")?;
    }
    if let Some(id) = input.building_id.as_deref() {
        require_ref(tx, "prj_building", id, "建筑物")?;
    }

    // R-70：可选文本统一按「去空白，全空写 NULL」归一，
    // 不再把 Some("") 这类空串落进 Option<String> 列。
    let delivery_date = norm_text(input.delivery_date);
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
        planned_start: norm_text(input.planned_start),
        planned_end: norm_text(input.planned_end),
        actual_start: None,
        actual_end: None,
        delivery_date,
        pricing_method: input.pricing_method,
        retention_rate_bp: input.retention_rate_bp,
        advance_rate_bp: input.advance_rate_bp,
        status: ContractStatus::Draft,
        payment_terms: input.payment_terms,
        remark: norm_text(input.remark),
        is_deleted: false,
        version: 1,
        created_at: now.clone(),
        created_by: Some(operator.to_string()),
        updated_at: now,
        updated_by: Some(operator.to_string()),
        bid_method: input.bid_method,
        open_bid_date,
        project_leader: norm_text(input.project_leader),
        contact_info: norm_text(input.contact_info),
        bid_agent_id,
    };
    c.validate()?;
    repo::insert_contract(tx, &c)?;
    db::insert_audit_log(
        tx,
        "ct_contract",
        &c.id,
        Some(&format!("{} {}", c.contract_no, c.contract_name)),
        "CREATE",
        Some(&format!("{} {}", c.contract_no, c.contract_name)),
        operator,
    )?;
    Ok(c)
}

/// 主档修改中的一个字段变更（R-51：保存成功后要逐条列给用户看）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    /// 字段中文名，如「所属项目」
    pub field: &'static str,
    /// 修改前的展示值（空值统一显示为「—」）
    pub old: String,
    /// 修改后的展示值
    pub new: String,
    /// 长文本（付款条款 / 备注）：审计只记「已修改」，成功弹窗仍显示全文
    pub long_text: bool,
}

impl FieldChange {
    /// 审计日志里的一行：长文本不透传原文，避免日志被整段条款淹没。
    pub fn audit_text(&self) -> String {
        if self.long_text {
            format!("{}：已修改", self.field)
        } else {
            format!("{}：{} → {}", self.field, self.old, self.new)
        }
    }
}

/// 主档修改的返回值：改后的合同 + 逐字段变更清单。
#[derive(Debug, Clone)]
pub struct ContractEdit {
    pub contract: Contract,
    pub changes: Vec<FieldChange>,
}

/// 修改合同主档信息（R-50）。
///
/// 与「新建」共用同一套档案解析规则（项目 / 承包单位 / 甲方 / 招标代理），
/// 保存时做三件事：字段级 diff → 写审计日志 → 乐观锁更新（`version` 不匹配即报冲突）。
///
/// **不碰**合同编号、合同金额、审计价、状态、交工日期：这些字段各有专门的
/// 留痕入口，从这里改会绕过历史表，审计链会断。
///
/// `reason` 必填：与「编辑金额」一致，任何主档变更都要能回答「为什么改」。
///
/// 返回 `ContractEdit`：界面用它弹「本次修改了什么」，日志用它落同一份 diff，
/// 两处不会出现两套口径。
pub fn update_contract_master(
    conn: &mut Connection,
    contract_id: &str,
    input: EditContract,
    reason: &str,
    operator: &str,
) -> Result<ContractEdit> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(DomainError::MissingField("修改原因").into());
    }
    if input.contract_name.trim().is_empty() {
        return Err(DomainError::MissingField("合同名称").into());
    }
    if input.payment_terms.trim().is_empty() {
        return Err(DomainError::MissingField("付款条款").into());
    }
    if input.contract_category_code.trim().is_empty() {
        return Err(DomainError::MissingField("合同类别").into());
    }
    validate_iso_date(&input.sign_date)?;

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut c = repo::get_contract(&tx, contract_id)?;
    // R-68（C2）：作废合同仍允许补充说明性信息（名称 / 备注 / 联系方式等）。
    // 计价类字段（金额 / 审计价 / 交工日期）本来就不在本入口的可改字段里，
    // 它们各自另走 `ensure_pricing_mutable` 守卫。已删除合同一律拒绝。
    if c.is_deleted {
        return Err(DomainError::IllegalState("合同已删除，不能修改信息".into()).into());
    }
    // R-68（C9）：类型变更会改变付清条件与封顶规则，必须先二次确认。
    if input.contract_type != c.contract_type && !input.confirm_type_change {
        let note = if input.contract_type == ContractType::Engineering {
            "改为工程类后，该合同将受「已审定 + 交工满 2 年」与 97% 封顶约束，已有付款可能变成超付"
        } else {
            "改为货物 / 服务类后，付清条件与封顶规则随之变化，请确认历史付款仍然合规"
        };
        return Err(DomainError::NeedConfirm(format!(
            "合同类型将由「{}」改为「{}」：{note}",
            c.contract_type.label(),
            input.contract_type.label()
        ))
        .into());
    }

    // 三个文本档案与新建同路径：命中就复用，未命中就自动建档
    let project_id = resolve_project(&tx, "", Some(&input.project_text), operator)?;
    let contractor_id = resolve_contractor(
        &tx,
        "",
        Some(&input.contractor_text),
        input.contract_type,
        operator,
    )?;
    let party_a = resolve_owner(&tx, &input.party_a, operator)?;
    let bid_agent_id = match input.bid_agent_text.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => Some(ensure_contractor(
            &tx,
            name,
            ContractorRole::Agent,
            operator,
        )?),
        _ => None,
    };

    // 字段级 diff：只记真正变化的项，避免「点保存就多一条流水」
    let mut changes: Vec<FieldChange> = Vec::new();
    macro_rules! note {
        ($label:expr, $old:expr, $new:expr) => {{
            let old = $old.to_string();
            let new = $new.to_string();
            if old != new {
                changes.push(FieldChange {
                    field: $label,
                    old,
                    new,
                    long_text: false,
                });
            }
        }};
    }
    // 长文本单独一支：审计只记「已修改」，成功弹窗仍显示前后全文
    macro_rules! note_long {
        ($label:expr, $old:expr, $new:expr) => {{
            if $old != $new {
                changes.push(FieldChange {
                    field: $label,
                    old: $old,
                    new: $new,
                    long_text: true,
                });
            }
        }};
    }
    let dash = |s: &Option<String>| s.clone().unwrap_or_else(|| "—".into());

    note!("合同名称", c.contract_name, input.contract_name);
    // 档案类字段按 id 判变化：用户可能只改了写法（名称 / 编码），指向同一个档案时不算修改
    if c.project_id != project_id {
        changes.push(FieldChange {
            field: "所属项目",
            old: project_name_of(&tx, &c.project_id),
            new: project_name_of(&tx, &project_id),
            long_text: false,
        });
    }
    if c.contractor_id != contractor_id {
        changes.push(FieldChange {
            field: "承包单位",
            old: contractor_label(&tx, &c.contractor_id),
            new: contractor_label(&tx, &contractor_id),
            long_text: false,
        });
    }
    note!("发包人", c.party_a, party_a);
    note!(
        "合同类型",
        c.contract_type.label(),
        input.contract_type.label()
    );
    // R-56：与界面下拉同口径显示中文名（工程款 / 设备款…），
    // 不把 GC / SB 这类内部编码直接递给用户
    let category_label = |code: &str| -> String {
        dict::contract_category_name(code)
            .unwrap_or(code)
            .to_string()
    };
    note!(
        "合同类别",
        category_label(&c.contract_category_code),
        category_label(input.contract_category_code.trim())
    );
    note!("签订日期", c.sign_date, input.sign_date.trim());
    note!(
        "工期",
        format!("{} 日历天", c.duration_days),
        format!("{} 日历天", input.duration_days)
    );
    note!(
        "计划开工",
        dash(&c.planned_start),
        dash(&input.planned_start)
    );
    note!("计划交工", dash(&c.planned_end), dash(&input.planned_end));
    note!(
        "计价方式",
        c.pricing_method.map(|x| x.label()).unwrap_or("—"),
        input.pricing_method.map(|x| x.label()).unwrap_or("—")
    );
    note!(
        "质保金率",
        // R-52：与表单（输入 3 表示 3%）、详情卡（3.00）同口径，不再输出 300 bp
        Rate::from_bp(c.retention_rate_bp).percent_string(),
        Rate::from_bp(input.retention_rate_bp).percent_string()
    );
    note!(
        "招标代理",
        bid_agent_label(&tx, c.bid_agent_id.as_deref()),
        bid_agent_label(&tx, bid_agent_id.as_deref())
    );
    // 长文本：审计只记「改过」，成功弹窗显示前后全文（`long_text` 标记）
    note_long!(
        "付款条款",
        c.payment_terms.clone(),
        input.payment_terms.clone()
    );
    note_long!("备注", dash(&c.remark), dash(&input.remark));
    note!(
        "招标方式",
        c.bid_method
            .as_ref()
            .map(|x| x.label())
            .unwrap_or_else(|| "—".to_string()),
        input
            .bid_method
            .as_ref()
            .map(|x| x.label())
            .unwrap_or_else(|| "—".to_string())
    );
    note!(
        "开标时间",
        dash(&c.open_bid_date),
        dash(&input.open_bid_date)
    );
    note!(
        "项目负责人",
        dash(&c.project_leader),
        dash(&input.project_leader)
    );
    note!("联系方式", dash(&c.contact_info), dash(&input.contact_info));

    if changes.is_empty() {
        // 不叫「口径违例」：这不是违规，只是提交值与库里一致
        return Err(DomainError::NoChange("没有修改任何内容，无需留痕".into()).into());
    }

    // 只落主档字段；编号 / 金额 / 审计价 / 状态 / 交工日期原样保留
    c.contract_name = input.contract_name.trim().to_string();
    c.project_id = project_id;
    c.contractor_id = contractor_id;
    c.party_a = party_a;
    c.contract_type = input.contract_type;
    c.contract_category_code = input.contract_category_code.trim().to_string();
    c.sign_date = input.sign_date.trim().to_string();
    c.duration_days = input.duration_days;
    c.planned_start = input.planned_start;
    c.planned_end = input.planned_end;
    c.pricing_method = input.pricing_method;
    c.retention_rate_bp = input.retention_rate_bp;
    c.payment_terms = input.payment_terms;
    c.remark = input.remark;
    // R-70：库里若含无法识别的招标方式原码，读路径会原样带回 Unknown；
    // 写库时由 `repo::legal_bid_code` 归一为 NULL，把原码记进留痕以便追溯，
    // 合同因此仍可编辑，且不会把非法码写进 CHECK 约束列。
    let unknown_bid: Option<String> = match &input.bid_method {
        Some(BidMethod::Unknown(raw)) => Some(raw.clone()),
        _ => None,
    };
    c.bid_method = input.bid_method;
    c.open_bid_date = input.open_bid_date;
    c.project_leader = input.project_leader;
    c.contact_info = input.contact_info;
    c.bid_agent_id = bid_agent_id;
    c.updated_at = now_iso();
    c.updated_by = Some(operator.to_string());

    // 乐观锁在这里生效：version 不匹配会返回「已被并发修改」，不会静默覆盖
    repo::update_contract(&tx, &c)?;
    // 库里是 `version = version + 1`，内存里的快照要跟上，
    // 否则返回值带着旧版本号，调用方再保存一次就会误报冲突。
    c.version += 1;

    let detail = format!(
        "{}｜{}{}",
        reason,
        changes
            .iter()
            .map(FieldChange::audit_text)
            .collect::<Vec<_>>()
            .join("；"),
        unknown_bid
            .map(|raw| format!("；招标方式原码「{raw}」无法识别，已置空，请重新选择"))
            .unwrap_or_default()
    );
    db::insert_audit_log(
        &tx,
        "ct_contract",
        &c.id,
        Some(&format!("{} {}", c.contract_no, c.contract_name)),
        "EDIT_CONTRACT",
        Some(&detail),
        operator,
    )?;
    tx.commit()?;
    Ok(ContractEdit {
        contract: c,
        changes,
    })
}

/// 项目名称（不含编码）：界面展示与「本次修改」弹窗都用它，
/// 与修改窗里所属项目的显示口径一致（R-51）。查不到给「—」，不留空白（P7）。
fn project_name_of(conn: &Connection, id: &str) -> String {
    repo::list_projects(conn)
        .ok()
        .and_then(|v| v.into_iter().find(|p| p.id == id))
        .map(|p| p.project_name)
        .unwrap_or_else(|| "—".to_string())
}

/// 单位展示名，查不到返回「—」（与其它缺失值口径一致，P7）。
fn contractor_label(conn: &Connection, id: &str) -> String {
    repo::list_contractors(conn)
        .ok()
        .and_then(|v| v.into_iter().find(|x| x.id == id))
        .map(|x| x.contractor_name)
        .unwrap_or_else(|| "—".to_string())
}

fn bid_agent_label(conn: &Connection, id: Option<&str>) -> String {
    match id {
        // 缺失值统一由 contractor_label 输出「—」，这里不再二次判断空串
        Some(id) => contractor_label(conn, id),
        None => "—".to_string(),
    }
}

/// 合同生效（草稿 → 生效）。本期无审批环节，由登记人直接生效。
///
/// 生效是「合同正本必须齐备」的卡点：存草稿阶段允许缺附件，避免手里没有
/// 电子件时连合同信息都录不进去；一旦要进入可拨款状态，就要求合同附件在位。
pub fn activate_contract(
    conn: &mut Connection,
    contract_id: &str,
    operator: &str,
) -> Result<Contract> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let c = activate_in_tx(&tx, contract_id, operator)?;
    tx.commit()?;
    Ok(c)
}

/// 事务内的合同生效主体（R-70：组合写入复用）。
fn activate_in_tx(tx: &Connection, contract_id: &str, operator: &str) -> Result<Contract> {
    let mut c = repo::get_contract(tx, contract_id)?;
    if !repo::has_attachment(tx, &c.id, AttachmentKind::Contract.code())? {
        return Err(DomainError::AttachmentRequired(AttachmentKind::Contract.label()).into());
    }
    c.transit(ContractStatus::Active)?;
    c.updated_at = now_iso();
    c.updated_by = Some(operator.to_string());
    repo::update_contract(tx, &c)?;
    db::insert_audit_log(
        tx,
        "ct_contract",
        &c.id,
        Some(&format!("{} {}", c.contract_no, c.contract_name)),
        "ACTIVATE",
        None,
        operator,
    )?;
    Ok(c)
}

// ---------------------------------------------------------------- 合同附件

/// 保存或替换某类附件（每类仅一份，冲突即覆盖）。
///
/// 文件本体写入 `ct_contract_attachment.content`，因此这里挡住三件事：
/// 空文件、超过 50 MB 的单文件、以及该合同附件总量超过 20 × 单文件上限
/// （R-39）——它们是数据库体积失控的主要来源。
///
/// 写附件与写审计日志在**同一个 IMMEDIATE 事务**里，避免出现「文件进了库、
/// 附件名净化：库里只存可安全显示的 basename。
///
/// 原始名可能带 `..\..\` / `/` / Windows 保留字符；「另存为」会把它当默认文件名
/// 交给系统对话框，拼进用户选的目录就可能写到目录外。这里统一取最后一段、
/// 去掉控制字符与保留字符，并限制长度。
fn sanitize_file_name(raw: &str) -> String {
    let base = raw.trim().rsplit(['/', '\\']).next().unwrap_or("").trim();
    let mut out = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            continue;
        }
        out.push(ch);
    }
    let out = out.trim();
    if out == "." || out == ".." {
        return String::new();
    }
    out.chars().take(180).collect()
}

/// 日志没记上」或反之的半截状态。
pub fn save_attachment(
    conn: &mut Connection,
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
    let name = sanitize_file_name(file_name);
    if name.is_empty() {
        return Err(DomainError::MissingField("附件文件名").into());
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let (saved, old_size) =
        save_attachment_in_tx(&tx, contract_id, kind, &name, mime_type, content, operator)?;
    tx.commit()?;
    // R-67：只有「换成更小的文件」才压缩。SQLite 只把旧 BLOB 占的页挂回 freelist，
    // 文件高水位不回落（10 MB 换成 1 MB，库仍是 10 MB）；变大时压缩纯属白等一次全库重写。
    // 压缩失败不影响「附件已保存」这个结果，下次替换 / 删除会再试。
    if old_size > content.len() as i64 {
        let _ = db::compact(conn);
    }
    Ok(saved)
}

/// 事务内的附件 upsert 主体（R-70：组合写入复用）。
///
/// 返回（落库记录，被覆盖旧文件的大小）；VACUUM 只能在事务外做，故由调用方决定。
fn save_attachment_in_tx(
    tx: &Connection,
    contract_id: &str,
    kind: AttachmentKind,
    name: &str,
    mime_type: Option<&str>,
    content: &[u8],
    operator: &str,
) -> Result<(Attachment, i64)> {
    // R-70：组合写入也走这条路径，空文件 / 超限 / 空文件名必须在这里挡住，
    // 不能只依赖单条上传入口的校验，否则会写进脏数据。
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
    if name.trim().is_empty() {
        return Err(DomainError::MissingField("附件文件名").into());
    }

    // 同类是「覆盖」，先扣掉旧行的体积再算总量，否则反复替换同一类附件会被误判为超限
    let kind_code = kind.code();
    let existing = repo::list_attachments(tx, contract_id)?;
    // R-67：记下被覆盖那一份的大小，提交后据此判断值不值得压缩
    let old_size = existing
        .iter()
        .find(|x| x.kind_code == kind_code)
        .map(|x| x.size_bytes)
        .unwrap_or(0);
    let other_total = existing
        .iter()
        .filter(|x| x.kind_code != kind_code)
        .fold(0_i64, |acc, x| acc.saturating_add(x.size_bytes));
    let total_cap = ctp_core::attachment::MAX_ATTACHMENT_TOTAL_BYTES;
    if other_total.saturating_add(content.len() as i64) > total_cap {
        return Err(StoreError::Inconsistent(format!(
            "该合同附件总量将达 {:.0} MB，超过 {} MB 上限，请先删除不用的附件",
            (other_total + content.len() as i64) as f64 / (1024.0 * 1024.0),
            total_cap / 1024 / 1024
        )));
    }

    let now = now_iso();
    let a = Attachment {
        id: ids::new_id("ATT"),
        contract_id: contract_id.to_string(),
        kind_code: kind_code.to_string(),
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
    repo::upsert_attachment(tx, &a, content)?;
    // 覆盖已有行时主键仍是旧 id（ON CONFLICT DO UPDATE 不换主键），
    // 先回读拿真实记录再写日志，否则日志会指向一条不存在的新 id（R-68 幽灵日志）。
    let saved = repo::list_attachments(tx, contract_id)?
        .into_iter()
        .find(|x| x.kind_code == kind_code);
    let logged = saved.as_ref().unwrap_or(&a);
    db::insert_audit_log(
        tx,
        "ct_contract_attachment",
        &logged.id,
        Some(name),
        "UPSERT",
        Some(&format!("{} · {}", kind.label(), name)),
        operator,
    )?;
    Ok((saved.unwrap_or(a), old_size))
}

pub fn list_attachments(conn: &Connection, contract_id: &str) -> Result<Vec<Attachment>> {
    repo::list_attachments(conn, contract_id)
}

pub fn attachment_bytes(conn: &Connection, id: &str) -> Result<Vec<u8>> {
    repo::attachment_bytes(conn, id)
}

/// 删除附件与删除留痕同事务（R-39）。
///
/// R-67：删完再压缩。附件是库里最大的对象，删掉 7 MB 扫描件后若不 VACUUM，
/// 文件大小一点都不会变，用户看到的就是「删了也不省空间」。
/// R-68（#21）：只在**真的删掉了**（命中 ≥ 1 行）才压缩；删不存在的 id
/// 不再白跑一次全库重写。
pub fn delete_attachment(conn: &mut Connection, id: &str, operator: &str) -> Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let deleted = repo::delete_attachment(&tx, id)?;
    let had_deleted = deleted.is_some();
    if let Some((kind, name)) = deleted {
        db::insert_audit_log(
            &tx,
            "ct_contract_attachment",
            id,
            Some(&name),
            "DELETE",
            Some(&format!("删除附件：{name}（{kind}）")),
            operator,
        )?;
    }
    tx.commit()?;
    if had_deleted {
        // 压缩失败不影响删除结果（记录已经没了），只是空间暂时还挂在 freelist 上
        let _ = db::compact(conn);
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
    db::insert_audit_log(
        &tx,
        "ct_contract",
        &c.id,
        Some(&format!("{} {}", c.contract_no, c.contract_name)),
        "VOID",
        None,
        operator,
    )?;
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
    ensure_pricing_mutable(&c, "编辑金额")?;
    let old = c.original_amount_cent;
    if old == new_amount {
        // R-52：不是口径问题，也不是「填错」，只是没有变化；不顶「口径违例」的帽子
        return Err(DomainError::NoChange("金额未变化，没有修改任何内容，无需留痕".into()).into());
    }
    // R-68：金额有下限——累计已付与审计价取大者。
    // 低于已付会让支付率倒挂（已付 > 基准），该合同可付额度转负、此后既不能拨款
    // 也不能靠再改金额回到可用状态；低于审计价则与审定结论冲突。
    // 必须在事务内读，金额校验与写入用同一份快照，避免两次读之间插入付款。
    let paid = repo::paid_net(&tx, &c.id)?;
    let audit = c.audit_price().map(|m| m.cent()).unwrap_or(0);
    let floor = paid.cent().max(audit);
    if new_amount.cent() < floor {
        return Err(DomainError::AmountBelowFloor {
            requested: new_amount.cent(),
            floor,
        }
        .into());
    }
    let at = now_iso();
    c.original_amount_cent = new_amount;
    c.updated_at = at.clone();
    c.updated_by = Some(operator.to_string());
    repo::update_contract(&tx, &c)?;

    // C8：变更历史进独立历史表（带合同 id、不限全局条数），
    // 取代 sys_meta 里跨合同共享 50 条上限的 JSON 数组
    repo::insert_change_history(
        &tx,
        &repo::ChangeHistoryEntry {
            contract_id: &c.id,
            change_type: "AMOUNT",
            old_cent: Some(old.cent()),
            new_cent: Some(new_amount.cent()),
            reason,
            operator_id: operator,
            created_at: &at,
        },
    )?;
    db::insert_audit_log(
        &tx,
        "ct_contract",
        &c.id,
        Some(&format!("{} {}", c.contract_no, c.contract_name)),
        "EDIT_AMOUNT",
        Some(&format!(
            "{} -> {}：{reason}",
            old.format_yuan(),
            new_amount.format_yuan()
        )),
        operator,
    )?;
    tx.commit()?;
    Ok(c)
}

/// 计价类字段（合同金额 / 审计价 / 交工日期）的变更守卫。
///
/// 统一成一处：作废与已删除合同一律拒绝，提示语带上具体动作名。
/// 之前每个入口各写一句，`register_audit_price` 就漏了作废判定——
/// 作废合同仍能被改审计价，连带改写视图里的基准与支付率。
fn ensure_pricing_mutable(c: &Contract, action: &str) -> Result<()> {
    if c.is_deleted {
        return Err(DomainError::IllegalState(format!("合同已删除，不能{action}")).into());
    }
    if c.status == ContractStatus::Void {
        return Err(DomainError::IllegalState(format!("作废合同不能{action}")).into());
    }
    Ok(())
}

/// 可选文本的落库口径：去首尾空白，全空视为「未填」。
fn norm_text(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 审计价登记 / 修改 / 清空（BR-14 / BR-16）。
///
/// * `price` 为 `None` 或 `<= 0` 即「未审定」，基准回退合同金额；
/// * 必填原因，写 `audit_price_history`；
/// * 只改档案与基准，**不改写任何历史拨款流水**（累计已付保持不变）。
// 业务字段本就较多，且调用点只有一个，保留扁平签名比引入参数对象更直观。
#[allow(clippy::too_many_arguments)]
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
    // R-68：作废 / 已删除合同不得再改审计价。审计价是计价基准，改它会连带改变
    // 视图里的基准金额与支付率，让作废合同重新参与台账与报表口径。
    ensure_pricing_mutable(&c, "登记审计价")?;
    let old = c.audit_price().map(|m| m.cent());
    // 归一：<= 0 一律按未审定处理（4.2.7）；字符串字段也先 trim 再落库，
    // 让「同值判定」和「实际写入」用同一份值，避免多打一个空格就多一条留痕（R-52）。
    let normalized = price.filter(|p| p.cent() > 0);
    let new_date = norm_text(audit_date);
    let new_org = norm_text(audit_org);
    let new_report = norm_text(audit_report_no);
    if c.audit_price() == normalized
        && norm_text(c.audit_date.clone()) == new_date
        && norm_text(c.audit_org.clone()) == new_org
        && norm_text(c.audit_report_no.clone()) == new_report
    {
        // 四个字段都没变：不写历史、不写日志、不改档案
        return Err(
            DomainError::NoChange("审计价信息未变化，没有修改任何内容，无需留痕".into()).into(),
        );
    }
    c.audit_price_cent = normalized;
    c.audit_date = new_date;
    c.audit_org = new_org;
    c.audit_report_no = new_report;
    c.updated_at = now_iso();
    c.updated_by = Some(operator.to_string());
    repo::update_contract(&tx, &c)?;

    repo::insert_change_history(
        &tx,
        &repo::ChangeHistoryEntry {
            contract_id: &c.id,
            change_type: "AUDIT_PRICE",
            old_cent: old,
            new_cent: normalized.map(|m| m.cent()),
            reason,
            operator_id: operator,
            created_at: &c.updated_at,
        },
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
    db::insert_audit_log(
        &tx,
        "ct_contract",
        &c.id,
        Some(&format!("{} {}", c.contract_no, c.contract_name)),
        "SET_AUDIT_PRICE",
        Some(&detail),
        operator,
    )?;
    tx.commit()?;
    Ok(c)
}

/// R-67：该合同最近一次登记审计价时填的「变更原因」，仅用于弹窗回填。
///
/// C8 起优先读变更历史表（按合同归属、不再被别的合同挤掉）；
/// 历史表没铺开前的旧数据回退读操作日志里最近一条 `SET_AUDIT_PRICE` 的
/// detail（格式「审计价 X（原 Y）：原因」），取最后一个中文冒号之后的部分。
pub fn last_audit_reason(conn: &Connection, contract_id: &str) -> Option<String> {
    if let Some(r) = repo::last_change_reason(conn, contract_id, "AUDIT_PRICE")
        .ok()
        .flatten()
    {
        return Some(r);
    }
    let detail: Option<String> = conn
        .query_row(
            "SELECT detail FROM sys_audit_log \
             WHERE target_type = 'ct_contract' AND target_id = ?1 AND action = 'SET_AUDIT_PRICE' \
             ORDER BY created_at DESC, id DESC LIMIT 1",
            [contract_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    detail
        .and_then(|d| {
            d.rsplit_once('：')
                .map(|(_, reason)| reason.trim().to_string())
        })
        .filter(|r| !r.is_empty())
}

/// 项目交工：登记 / 修改交工日期（覆盖写，写审计日志）。
pub fn set_delivery_date(
    conn: &mut Connection,
    contract_id: &str,
    date: &str,
    operator: &str,
) -> Result<Contract> {
    validate_iso_date(date)?;
    let d = date.trim();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut c = repo::get_contract(&tx, contract_id)?;
    // 交工日期直接决定「交工满 2 年」的付清条件，与金额同级，同样不许在作废后改
    ensure_pricing_mutable(&c, "登记交工日期")?;
    let old = c.delivery_date.clone();
    if old.as_deref().map(str::trim) == Some(d) {
        // 同日期重复提交：不写日志、不改档案（R-52 / P4）
        return Err(
            DomainError::NoChange("交工日期未变化，没有修改任何内容，无需留痕".into()).into(),
        );
    }
    c.delivery_date = Some(d.to_string());
    c.updated_at = now_iso();
    c.updated_by = Some(operator.to_string());
    repo::update_contract(&tx, &c)?;
    let detail = format!(
        "交工日期 {}（原 {}）",
        d,
        old.unwrap_or_else(|| "未登记".into())
    );
    db::insert_audit_log(
        &tx,
        "ct_contract",
        &c.id,
        Some(&format!("{} {}", c.contract_no, c.contract_name)),
        "SET_DELIVERY_DATE",
        Some(&detail),
        operator,
    )?;
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
    let import_key = input.import_key.clone();

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // 导入幂等：查重与写入在同一 IMMEDIATE 事务里，双实例并发也不会重复入账
    // （唯一索引 ux_payment_import_key 是最后一道闸）。
    if let Some(ref key) = import_key {
        let taken: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pay_payment WHERE import_key = ?1",
            [key],
            |r| r.get(0),
        )?;
        if taken > 0 {
            return Err(StoreError::Inconsistent(format!(
                "导入行 {key} 已入账，拒绝重复登记"
            )));
        }
    }
    let mut c = repo::get_contract(&tx, &input.contract_id)?;
    let paid = repo::paid_net(&tx, &c.id)?;
    // 超付容差固定为 0（R-33）：不再读 sys_meta.tol_bp，也不提供任何写入口
    let tolerance = Rate::ZERO;
    // 1) 合同须可拨款；2) 本次金额不得超过「付清条件」下的可付额度
    let quota = PaymentQuota::for_new_payment(&c, paid, tolerance, &input.pay_date)?;
    quota.check_new_payment(input.amount)?;

    let now = now_iso();
    let year = year_of(&input.pay_date);
    // R-68（C7）：与合同号同款兜底——唯一索引 ux_payment_no 命中时不再让整个
    // 事务（连同取号）回滚，跳过重取，5 次仍撞号才报数据异常。
    let (seq, payment_no) = {
        let mut got = None;
        for _ in 0..5 {
            let s = db::next_sequence(&tx, &format!("payment_no:{year}"))?;
            let no = ids::payment_no(&year, s);
            let taken: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pay_payment WHERE payment_no = ?1 AND is_deleted = 0",
                [&no],
                |r| r.get(0),
            )?;
            if taken == 0 {
                got = Some((s, no));
                break;
            }
        }
        got.ok_or_else(|| {
            StoreError::Inconsistent(format!(
                "拨款单号连续 5 次撞号（{year} 年度），请检查 pay_payment 与 sys_sequence 是否一致"
            ))
        })?
    };

    let p = Payment {
        id: ids::new_id("PM"),
        payment_no,
        title: norm_text(input.title),
        sort_no: seq,
        period_no: seq,
        contract_id: c.id.clone(),
        project_id: c.project_id.clone(),
        pay_date: input.pay_date,
        amount_cent: input.amount,
        reason: norm_text(input.reason),
        pay_method: input.pay_method,
        bank_serial_no: norm_text(input.bank_serial_no),
        pay_account: norm_text(input.pay_account),
        receive_account: norm_text(input.receive_account),
        status: PaymentStatus::Paid,
        operator_id: operator.to_string(),
        remark: norm_text(input.remark),
        is_deleted: false,
        version: 1,
        created_at: now.clone(),
        created_by: Some(operator.to_string()),
        updated_at: now,
        updated_by: Some(operator.to_string()),
    };
    p.validate()?;
    repo::insert_payment(&tx, &p)?;
    // 幂等键不进 Payment 实体（只有导入链路需要），插完立刻在同一事务补写；
    // 作废 / 编辑走的 update_payment 不含该列，键不会被抹掉。
    if let Some(ref key) = import_key {
        tx.execute(
            "UPDATE pay_payment SET import_key = ?2 WHERE id = ?1",
            rusqlite::params![p.id, key],
        )?;
    }

    // 首笔拨款：生效 → 履行中（4.2.3）
    if c.status == ContractStatus::Active {
        c.transit(ContractStatus::Performing)?;
        c.updated_at = p.created_at.clone();
        c.updated_by = Some(operator.to_string());
        repo::update_contract(&tx, &c)?;
        // R-70：首笔拨款触发的状态迁移也单独留痕，与作废回退对称。
        db::insert_audit_log(
            &tx,
            "ct_contract",
            &c.id,
            Some(&format!("{} {}", c.contract_no, c.contract_name)),
            "PERFORMING",
            Some(&format!(
                "登记首笔拨款 {}，合同状态由「生效」转为「履行中」",
                p.payment_no
            )),
            operator,
        )?;
    }

    db::insert_audit_log(
        &tx,
        "pay_payment",
        &p.id,
        Some(&format!("{}（合同 {}）", p.payment_no, c.contract_no)),
        "REGISTER",
        Some(&format!(
            "{} {} 元（合同 {}，已付 {} / 基准 {}）",
            p.payment_no,
            p.amount_cent.format_yuan(),
            c.contract_no,
            paid.checked_add(p.amount_cent)?.format_yuan(),
            quota.base_amount.format_yuan()
        )),
        operator,
    )?;
    tx.commit()?;
    Ok(p)
}

/// 拨款单作废（评审 H5：以「作废 + 重登」替代红冲）。作废后不再计入已付净额。
///
/// R-68（C15）：作废的是该合同**最后一笔**已付单据时，合同从「履行中」回退到
/// 「生效」——状态与事实保持一致；该状态迁移已加入状态机白名单。
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
        Some(&p.payment_no),
        "VOID",
        Some(&format!("作废 {}：{reason}", p.payment_no)),
        operator,
    )?;
    // C15：已付清零的「履行中」合同回退「生效」；回退本身也留痕
    let mut c = repo::get_contract(&tx, &p.contract_id)?;
    if c.status == ContractStatus::Performing && repo::paid_payment_count(&tx, &c.id)? == 0 {
        c.revert_performing_to_active()?;
        c.updated_at = p.updated_at.clone();
        c.updated_by = Some(operator.to_string());
        repo::update_contract(&tx, &c)?;
        db::insert_audit_log(
            &tx,
            "ct_contract",
            &c.id,
            Some(&format!("{} {}", c.contract_no, c.contract_name)),
            "REVERT_ACTIVE",
            Some(&format!(
                "作废最后一笔已付 {}，合同回退「生效」：{reason}",
                p.payment_no
            )),
            operator,
        )?;
    }
    tx.commit()?;
    Ok(p)
}

// ---------------------------------------------------------------- 查询与对账

/// 当前可付额度（展示用，不在事务内加锁）。按「今天」判定付清条件。
pub fn quota_of(conn: &Connection, contract_id: &str) -> Result<PaymentQuota> {
    quota_on(conn, contract_id, &today_local_iso())
}

/// 指定开单日期的可付额度：界面按表单里填的开单日期实时展示，
/// 与 `register_payment` 的校验口径完全一致（交工满 2 年按开单日判定）。
pub fn quota_on(conn: &Connection, contract_id: &str, pay_date: &str) -> Result<PaymentQuota> {
    let c = repo::get_contract(conn, contract_id)?;
    let paid = repo::paid_net(conn, contract_id)?;
    let tolerance = Rate::ZERO;
    let today = today_local_iso();
    let date = if ctp_core::date::is_iso_date(pay_date) {
        pay_date
    } else {
        today.as_str()
    };
    Ok(PaymentQuota::compute(
        &c,
        paid,
        Money::ZERO,
        tolerance,
        date,
    )?)
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

// ---------------------------------------------------------------- 报表导出

/// 汇总统计：按维度 + 筛选条件取数（R-32）。
pub fn summary_of(
    conn: &Connection,
    dim: ctp_core::ledger::SummaryDim,
    filter: &ctp_core::ledger::SummaryFilter,
) -> Result<crate::report::ReportResult> {
    let rows = repo::payment_summary(conn, dim, filter)?;
    Ok(crate::report::ReportResult::summary(
        &rows,
        dim,
        &filter.from,
        &filter.to,
        &filter_note(filter),
    ))
}

/// 把筛选条件写成一句人话（报表副标题与界面提示共用）。
/// 时间区间不进这句话——副标题里已有「统计区间」。
pub fn filter_note(f: &ctp_core::ledger::SummaryFilter) -> String {
    use ctp_core::contract::ContractType;
    let mut parts: Vec<String> = Vec::new();
    match f.archived {
        Some(true) => parts.push("归档关闭".into()),
        Some(false) => parts.push("履行中".into()),
        None => {}
    }
    if let Some(code) = f.contract_type.as_deref().filter(|s| !s.is_empty()) {
        if let Some(t) = ContractType::from_code(code) {
            parts.push(t.label().into());
        }
    }
    let kw = f.keyword.trim();
    if !kw.is_empty() {
        parts.push(format!("「{kw}」"));
    }
    parts.join(" · ")
}

/// 从视图行构造报表结果（屏幕表格与导出共用这一份）。
pub fn report_of(
    conn: &Connection,
    kind: crate::report::ReportKind,
) -> Result<crate::report::ReportResult> {
    use crate::report::{ReportKind, ReportResult};
    use ctp_core::ledger::SummaryDim;
    match kind {
        ReportKind::CategorySummary => Ok(ReportResult::category(&repo::category_summary(conn)?)),
        ReportKind::PeriodSummary => Ok(ReportResult::period(&repo::period_summary(conn)?)),
        // 汇总维度需要日期区间：CLI 走这里时用「本年年初 ~ 今天」+ 全量筛选，
        // 界面自己带上用户选的区间与筛选，走 `summary_of`
        ReportKind::SummaryProject => summary_of(
            conn,
            SummaryDim::Project,
            &range_filter(default_summary_range()),
        ),
        ReportKind::SummaryContractor => summary_of(
            conn,
            SummaryDim::Contractor,
            &range_filter(default_summary_range()),
        ),
        ReportKind::SummaryContract => summary_of(
            conn,
            SummaryDim::Contract,
            &range_filter(default_summary_range()),
        ),
        ReportKind::SummaryContractType => summary_of(
            conn,
            SummaryDim::ContractType,
            &range_filter(default_summary_range()),
        ),
        // 台账：CLI 与默认导出口径取全量；界面按筛选 / 选列自己构造结果
        ReportKind::ContractLedger => Ok(crate::report::ReportResult::contract_ledger(
            &repo::summaries(conn)?,
            crate::report::LedgerColumns::FULL,
        )),
        // 拨款一览：导出要带「合同名称 / 承包单位」，按 contract_id 关联 V1 汇总行；
        // 查不到的记录留空，不影响导出。
        ReportKind::PaymentLedger => {
            let payments = repo::list_payments(conn, None)?;
            let lookup = crate::report::payment_contract_lookup(&repo::summaries(conn)?);
            Ok(crate::report::ReportResult::payment_ledger_export(
                &payments,
                "全部合同",
                &lookup,
            ))
        }
    }
}

/// 把 `(from, to)` 变成「只限区间、其余不限」的筛选条件。
fn range_filter((from, to): (String, String)) -> ctp_core::ledger::SummaryFilter {
    ctp_core::ledger::SummaryFilter {
        from,
        to,
        ..Default::default()
    }
}

/// 汇总统计的默认区间：本年 1 月 1 日 ~ 今天（东八区，R-68 / C6）。
pub fn default_summary_range() -> (String, String) {
    let today = today_local_iso();
    let year = today.get(0..4).unwrap_or("2026");
    (format!("{year}-01-01"), today)
}

/// 导出一张报表到 .xlsx，并写一条 `EXPORT` 审计日志（FR-SYS-07）。
///
/// 只用 `&Connection`：导出不改业务数据，审计日志是普通 INSERT，不需要 `BEGIN IMMEDIATE`。
/// R-70：本模块是 GUI「只读查看 + 系统留痕」门面唯一允许调用的服务，
/// 只允许写 `sys_audit_log`，不得在此写入任何业务表。
pub fn export_report(
    conn: &Connection,
    kind: crate::report::ReportKind,
    path: &std::path::Path,
    operator: &str,
) -> Result<(usize, usize)> {
    let result = report_of(conn, kind)?;
    write_and_log(conn, &result, path, operator, None)
}

/// 导出**已经算好**的报表结果。
///
/// 汇总统计的区间由界面决定，不能走 `report_of` 的默认区间，所以单独开这个入口；
/// 写出的文件内容与屏幕表格同源（同一个 `ReportResult`）。
pub fn export_result(
    conn: &Connection,
    result: &crate::report::ReportResult,
    path: &std::path::Path,
    operator: &str,
) -> Result<(usize, usize)> {
    write_and_log(conn, result, path, operator, Some(&result.subtitle))
}

/// 两个导出入口共用的「写 .xlsx + 落审计日志」。
///
/// `subtitle` 只影响审计 detail 里是否多记一句口径说明，保持两个入口原有的日志内容不变；
/// 写文件、行列数、日志字段、操作者都只有这一处实现。
fn write_and_log(
    conn: &Connection,
    result: &crate::report::ReportResult,
    path: &std::path::Path,
    operator: &str,
    subtitle: Option<&str>,
) -> Result<(usize, usize)> {
    let (rows, cols) = crate::xlsx::write_report(result, path, operator)?;
    let mut detail = serde_json::json!({
        "report": result.kind.code(),
        "file": path.to_string_lossy(),
        "rows": rows,
        "columns": cols,
    });
    if let Some(s) = subtitle {
        detail["subtitle"] = serde_json::Value::String(s.to_string());
    }
    db::insert_audit_log(
        conn,
        "report",
        result.kind.code(),
        Some(result.kind.title()),
        "EXPORT",
        Some(&detail.to_string()),
        operator,
    )?;
    Ok((rows, cols))
}

// ---------------------------------------------------------------- 付款打印

/// 选中一张拨款单 → 解析 9 个域的取值（界面预览用）。
pub fn payment_print_data(
    conn: &Connection,
    payment_id: &str,
) -> std::result::Result<crate::print::PaymentPrintData, crate::print::PrintError> {
    crate::print::resolve(conn, payment_id)
}

/// 解析一批拨款单的版面数据：单张传一个 id，批量按顺序传多个 id。
///
/// 作废一律拒绝、顺序按传入顺序，口径见 `print::vouchers`。
/// 打印与预览都从这里取数，不存在第二套口径。
pub fn print_vouchers(
    conn: &Connection,
    payment_ids: &[String],
) -> std::result::Result<Vec<crate::paper::Voucher>, crate::print::PrintError> {
    crate::print::vouchers(conn, payment_ids)
}
