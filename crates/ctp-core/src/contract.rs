use crate::date::{is_iso_date, validate_opt_date};
use crate::dict;
use crate::error::{DomainError, Result};
use crate::money::Money;

/// 合同业务字段清单（评审 M1 的**唯一计数口径**）。
///
/// 文档正文在 4.2.2 / 2.9 / C-10 / 11.8.1 处给出 18 与 19 两个数字，实体清点为 20，
/// DDL 实列更多。本清单定义：「业务字段 = 用户在合同表单上录入或可编辑、且非派生的字段」，
/// 共 **31** 个（含 4 个审计价字段、含 `payment_terms`，不含 `id`、`status`、审计列与逻辑删除列）。
/// 任何字段增减只改这里，验收与测试都引用 `BUSINESS_FIELDS.len()`。
pub const BUSINESS_FIELDS: &[&str] = &[
    "contract_no",
    "contract_name",
    "project_id",
    "building_id",
    "contract_type",
    "contract_category_code",
    "category_id",
    "party_a",
    "contractor_id",
    "sign_date",
    "original_amount_cent",
    "audit_price_cent",
    "audit_date",
    "audit_org",
    "audit_report_no",
    "duration_days",
    "planned_start",
    "planned_end",
    "actual_start",
    "actual_end",
    "delivery_date",
    "pricing_method",
    "retention_rate_bp",
    "advance_rate_bp",
    "payment_terms",
    "remark",
    "bid_method",
    "open_bid_date",
    "project_leader",
    "contact_info",
    "bid_agent_id",
];

/// 业务字段计数（文档中一切「合同字段数」均应引用本函数，不再手写数字）。
pub fn business_field_count() -> usize {
    BUSINESS_FIELDS.len()
}

/// 合同类型（4.2.2）。
///
/// R-57 起只保留 **工程 / 货物 / 服务** 三类：原「施工」改名为「工程」，
/// 「监理 / 设计 / 勘察」三类整体退场。本软件尚未正式投用、不提供迁移，
/// 旧码既不在 DDL 的 CHECK 里，也不会被 `from_code` 接受。
///
/// 落库码沿用 `CONSTRUCT`，保持与界面文案「工程」的稳定映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContractType {
    #[default]
    Engineering,
    Supply,
    Service,
}

impl ContractType {
    /// 全部类型，顺序即下拉与筛选的展示顺序（单一来源，新增类型只改这里）。
    pub const ALL: [ContractType; 3] = [
        ContractType::Engineering,
        ContractType::Supply,
        ContractType::Service,
    ];

    pub fn code(self) -> &'static str {
        match self {
            ContractType::Engineering => "CONSTRUCT",
            ContractType::Supply => "SUPPLY",
            ContractType::Service => "SERVICE",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ContractType::Engineering => "工程",
            ContractType::Supply => "货物",
            ContractType::Service => "服务",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "CONSTRUCT" => ContractType::Engineering,
            "SUPPLY" => ContractType::Supply,
            "SERVICE" => ContractType::Service,
            _ => return None,
        })
    }
}

/// 计价方式（4.2.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PricingMethod {
    #[default]
    LumpSum,
    UnitPrice,
    CostPlus,
}

impl PricingMethod {
    pub fn code(self) -> &'static str {
        match self {
            PricingMethod::LumpSum => "LUMP_SUM",
            PricingMethod::UnitPrice => "UNIT_PRICE",
            PricingMethod::CostPlus => "COST_PLUS",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PricingMethod::LumpSum => "总价",
            PricingMethod::UnitPrice => "单价",
            PricingMethod::CostPlus => "成本加酬金",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "LUMP_SUM" => PricingMethod::LumpSum,
            "UNIT_PRICE" => PricingMethod::UnitPrice,
            "COST_PLUS" => PricingMethod::CostPlus,
            _ => return None,
        })
    }
}

/// 招标方式（新建合同页下拉，3 个固定值）。
///
/// R-68（C3）：新增 [`BidMethod::Unknown`] —— 库里出现无法识别的编码时**保留原码**，
/// 不再静默读成 `None`（`None` 会被下一次整表保存写成 NULL，造成无痕数据丢失）。
/// 因此本枚举不再 `Copy`；写回时 `Unknown` 原样落库，界面显示「未知(XXX)」。
/// 新库的 CHECK 约束本就拒绝未知码，这里属于纵深防御（防手工改库 / 约束被绕过）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BidMethod {
    /// 公开招标
    #[default]
    Open,
    /// 政府采购
    Government,
    /// 直接发包
    Direct,
    /// 未识别的历史编码，`String` 为原码
    Unknown(String),
}

impl BidMethod {
    pub fn code(&self) -> &str {
        match self {
            BidMethod::Open => "OPEN",
            BidMethod::Government => "GOV_PROC",
            BidMethod::Direct => "DIRECT",
            BidMethod::Unknown(raw) => raw.as_str(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            BidMethod::Open => "公开招标".into(),
            BidMethod::Government => "政府采购".into(),
            BidMethod::Direct => "直接发包".into(),
            BidMethod::Unknown(raw) => format!("未知({raw})"),
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "OPEN" => BidMethod::Open,
            "GOV_PROC" => BidMethod::Government,
            "DIRECT" => BidMethod::Direct,
            _ => return None,
        })
    }

    /// 读路径专用：未知编码不丢弃，包成 [`BidMethod::Unknown`] 原样带回，
    /// 让「读 → 改别的字段 → 保存」一轮下来不丢这份数据。
    pub fn from_code_keep(s: &str) -> Self {
        BidMethod::from_code(s).unwrap_or_else(|| BidMethod::Unknown(s.to_string()))
    }
}

/// 合同状态（4.2.3）。
///
/// 枚举保留 DDL 全集以便库内数据往返，但**本期只产生 5 态**（评审 H3：无审批、无结算）：
/// `DRAFT / ACTIVE / PERFORMING / COMPLETED / VOID`。`PENDING`（审批）、`SETTLED`、`CLOSED`
/// 属二期或已移出模块，任何进入它们的流转都被 `can_transition_to` 拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContractStatus {
    #[default]
    Draft,
    Pending,
    Active,
    Performing,
    Completed,
    Settled,
    Closed,
    Void,
}

impl ContractStatus {
    pub const PHASE_STATUSES: &'static [ContractStatus] = &[
        ContractStatus::Draft,
        ContractStatus::Active,
        ContractStatus::Performing,
        ContractStatus::Completed,
        ContractStatus::Void,
    ];

    pub fn code(self) -> &'static str {
        match self {
            ContractStatus::Draft => "DRAFT",
            ContractStatus::Pending => "PENDING",
            ContractStatus::Active => "ACTIVE",
            ContractStatus::Performing => "PERFORMING",
            ContractStatus::Completed => "COMPLETED",
            ContractStatus::Settled => "SETTLED",
            ContractStatus::Closed => "CLOSED",
            ContractStatus::Void => "VOID",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ContractStatus::Draft => "草稿",
            ContractStatus::Pending => "待审",
            ContractStatus::Active => "生效",
            ContractStatus::Performing => "履行中",
            ContractStatus::Completed => "已完工",
            ContractStatus::Settled => "结算完成",
            ContractStatus::Closed => "归档关闭",
            ContractStatus::Void => "作废",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "DRAFT" => ContractStatus::Draft,
            "PENDING" => ContractStatus::Pending,
            "ACTIVE" => ContractStatus::Active,
            "PERFORMING" => ContractStatus::Performing,
            "COMPLETED" => ContractStatus::Completed,
            "SETTLED" => ContractStatus::Settled,
            "CLOSED" => ContractStatus::Closed,
            "VOID" => ContractStatus::Void,
            _ => return None,
        })
    }

    /// 本期是否实际产生该状态。
    pub fn in_phase(self) -> bool {
        Self::PHASE_STATUSES.contains(&self)
    }

    /// 可拨款状态（4.2.2 `is_payable`）。
    pub fn is_payable(self) -> bool {
        matches!(
            self,
            ContractStatus::Active | ContractStatus::Performing | ContractStatus::Completed
        )
    }

    /// 本期状态机（去掉了审批与结算分支）。
    pub fn can_transition_to(self, target: ContractStatus) -> bool {
        use ContractStatus::*;
        if !target.in_phase() {
            return false;
        }
        matches!(
            (self, target),
            (Draft, Active)
                | (Draft, Void)
                | (Active, Performing)
                | (Active, Completed)
                | (Active, Void)
                | (Performing, Completed)
        )
    }
}

/// 合同实体，字段与 `ct_contract` 一一对应（5.4.3）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contract {
    pub id: String,
    pub project_id: String,
    pub contract_no: String,
    pub contract_name: String,
    pub contract_type: ContractType,
    /// 合同类别（业务口径，`sys_dict_item:CONTRACT_CATEGORY`），评审 H4
    pub contract_category_code: String,
    /// 会计分类（财务口径，`ct_category`），评审 H4；None = 未分类
    pub category_id: Option<String>,
    pub party_a: String,
    pub contractor_id: String,
    pub building_id: Option<String>,
    pub sort_no: i64,
    pub sign_date: String,
    pub original_amount_cent: Money,
    /// 审计价（分）：None / <= 0 = 未审定；> 0 即切换有效计价基准（BR-14）
    pub audit_price_cent: Option<Money>,
    pub audit_date: Option<String>,
    pub audit_org: Option<String>,
    pub audit_report_no: Option<String>,
    pub duration_days: i64,
    pub planned_start: Option<String>,
    pub planned_end: Option<String>,
    pub actual_start: Option<String>,
    pub actual_end: Option<String>,
    /// 交工日期（项目交工登记，YYYY-MM-DD）
    pub delivery_date: Option<String>,
    pub pricing_method: Option<PricingMethod>,
    pub retention_rate_bp: i32,
    pub advance_rate_bp: i32,
    pub status: ContractStatus,
    /// 付款条款（FR-CT-01 必录项，评审 M1 指出原实体缺失该字段）
    pub payment_terms: String,
    pub remark: Option<String>,
    pub is_deleted: bool,
    pub version: i64,
    pub created_at: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
    // —— 新建合同页扩展字段（评审后增补，全部选填）——
    /// 招标方式（公开招标 / 政府采购 / 直接发包）
    pub bid_method: Option<BidMethod>,
    /// 开标时间（YYYY-MM-DD）
    pub open_bid_date: Option<String>,
    /// 项目负责人
    pub project_leader: Option<String>,
    /// 联系方式
    pub contact_info: Option<String>,
    /// 招标代理单位 id（自动建档进 `org_contractor`，角色 AGENT）
    pub bid_agent_id: Option<String>,
}

impl Contract {
    /// 合同金额（暂定）≡ 原始签约金额（V1.6 起可直接编辑，不再叠加变更）。
    pub fn total_amount(&self) -> Money {
        self.original_amount_cent
    }

    /// 已审定时的审计价（> 0 才算审定）。
    pub fn audit_price(&self) -> Option<Money> {
        match self.audit_price_cent {
            Some(a) if a.cent() > 0 => Some(a),
            _ => None,
        }
    }

    pub fn is_audited(&self) -> bool {
        self.audit_price().is_some()
    }

    /// 有效计价基准（BR-14）：审计价 > 0 时**完全替代**合同金额。
    pub fn effective_base(&self) -> Money {
        self.audit_price().unwrap_or_else(|| self.total_amount())
    }

    /// 审减额（派生展示，不落盘；审增时为负）。
    pub fn audit_reduction(&self) -> Money {
        if self.is_audited() {
            self.total_amount().saturating_sub(self.effective_base())
        } else {
            Money::ZERO
        }
    }

    pub fn is_payable(&self) -> bool {
        self.status.is_payable() && !self.is_deleted
    }

    /// 本期状态流转；目标态不在本期范围时给出明确错误而非静默失败。
    pub fn transit(&mut self, target: ContractStatus) -> Result<()> {
        if !target.in_phase() {
            return Err(DomainError::StatusOutOfPhase(target.code().to_string()));
        }
        if !self.status.can_transition_to(target) {
            return Err(DomainError::IllegalState(format!(
                "合同状态 {} -> {} 不允许",
                self.status.code(),
                target.code()
            )));
        }
        self.status = target;
        Ok(())
    }

    /// R-68（C15）：作废最后一笔已付单据后，把「履行中」补偿回退为「生效」。
    ///
    /// 刻意**不进** `can_transition_to`：`transit(Active)` 是「生效」入口的通用
    /// 迁移规则，一旦放行 `Performing → Active`，`activate_contract` 就会允许
    /// 对已履行中的合同重复生效。回退是一次性补偿动作，只由 `void_payment`
    /// 在「已付笔数归零」时调用，单独走这里。
    pub fn revert_performing_to_active(&mut self) -> Result<()> {
        if self.status != ContractStatus::Performing {
            return Err(DomainError::IllegalState(format!(
                "只有履行中的合同可回退生效，当前 {}",
                self.status.code()
            )));
        }
        self.status = ContractStatus::Active;
        Ok(())
    }

    /// 保存前校验：覆盖 FR-CT-01 必录项、日期合法性、比率区间与时间倒序。
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(DomainError::MissingField("id"));
        }
        if self.contract_no.trim().is_empty() {
            return Err(DomainError::MissingField("合同编号"));
        }
        if self.contract_name.trim().is_empty() {
            return Err(DomainError::MissingField("合同名称"));
        }
        if self.project_id.trim().is_empty() {
            return Err(DomainError::MissingField("项目"));
        }
        if self.contractor_id.trim().is_empty() {
            return Err(DomainError::MissingField("承包单位"));
        }
        if self.party_a.trim().is_empty() {
            return Err(DomainError::MissingField("甲方（建设单位）"));
        }
        if self.payment_terms.trim().is_empty() {
            return Err(DomainError::MissingField("付款条款"));
        }
        if !dict::is_valid_contract_category(&self.contract_category_code) {
            return Err(DomainError::InvalidDict(format!(
                "合同类别 {}（应为 GC/SB/CL/FW/JC）",
                self.contract_category_code
            )));
        }
        if let Some(cat) = &self.category_id {
            if cat.trim().is_empty() {
                return Err(DomainError::InvalidDict("会计分类为空字符串".into()));
            }
        }
        if self.original_amount_cent.cent() < 0 {
            return Err(DomainError::NegativeAmount(
                self.original_amount_cent.cent(),
            ));
        }
        if let Some(a) = self.audit_price_cent {
            if a.cent() < 0 {
                return Err(DomainError::NegativeAmount(a.cent()));
            }
        }
        if !(0..=10_000).contains(&self.retention_rate_bp) {
            return Err(DomainError::Invariant(format!(
                "质保金比例越界：{} bp",
                self.retention_rate_bp
            )));
        }
        if !(0..=10_000).contains(&self.advance_rate_bp) {
            return Err(DomainError::Invariant(format!(
                "预付款比例越界：{} bp",
                self.advance_rate_bp
            )));
        }
        if self.duration_days < 0 {
            return Err(DomainError::Invariant("合同工期不能为负".into()));
        }
        if !is_iso_date(&self.sign_date) {
            return Err(DomainError::DateFormat(self.sign_date.clone()));
        }
        validate_opt_date(self.audit_date.as_deref())?;
        validate_opt_date(self.open_bid_date.as_deref())?;
        validate_opt_date(self.planned_start.as_deref())?;
        validate_opt_date(self.planned_end.as_deref())?;
        validate_opt_date(self.actual_start.as_deref())?;
        validate_opt_date(self.actual_end.as_deref())?;
        validate_opt_date(self.delivery_date.as_deref())?;
        if let (Some(s), Some(e)) = (&self.planned_start, &self.planned_end) {
            if e < s {
                return Err(DomainError::Invariant(format!(
                    "计划交工 {e} 早于计划开工 {s}"
                )));
            }
        }
        if !self.status.in_phase() {
            return Err(DomainError::StatusOutOfPhase(
                self.status.code().to_string(),
            ));
        }
        // Unknown 只用于「读到一个不认识的旧码」，不能原样写回：
        // DDL 的 CHECK 只收 OPEN / GOV_PROC / DIRECT，硬写会拿到一条 SQLite 约束错误。
        if let Some(BidMethod::Unknown(raw)) = &self.bid_method {
            return Err(DomainError::Invariant(format!(
                "招标方式编码「{raw}」无法识别，不能写回数据库"
            )));
        }
        Ok(())
    }
}

/// 计划交工日 = 计划开工日 + 工期（**含开工当日**，R-50）。
///
/// 例：`2026-03-01` 开工、工期 90 日历天 → `2026-05-29`（开工当天算第 1 天）。
///
/// 工期 `<= 0`、开工日期不是合法 `YYYY-MM-DD`、或结果超出四位年份时返回 `None`；
/// 界面据此「不触发自动计算」，保持用户已填的值不动。
///
/// 与 `Contract::validate` 的 `planned_end >= planned_start` 天然相容：
/// 工期 1 天时交工日等于开工日。
pub fn planned_end_for(planned_start: &str, duration_days: i64) -> Option<String> {
    if duration_days <= 0 {
        return None;
    }
    crate::date::plus_days(planned_start.trim(), duration_days - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R-68（C3）：未知招标方式码读出来不能被静默丢成 None，
    /// 否则用户只改个备注点保存，就会把原值覆盖成 NULL 且审计里没有痕迹。
    #[test]
    fn 未知招标方式码原样往返() {
        let m = BidMethod::from_code_keep("LEGACY_X");
        assert_eq!(m, BidMethod::Unknown("LEGACY_X".into()));
        assert_eq!(m.code(), "LEGACY_X", "写回时必须是原码");
        assert_eq!(m.label(), "未知(LEGACY_X)");
        // 已知码不受影响
        assert_eq!(BidMethod::from_code_keep("GOV_PROC"), BidMethod::Government);
        assert_eq!(BidMethod::from_code("LEGACY_X"), None);
    }

    /// C15：回退是补偿动作，不能把「生效」入口的通用状态机放宽。
    #[test]
    fn 状态回退不走通用迁移表() {
        use ContractStatus::*;
        assert!(
            !Performing.can_transition_to(Active),
            "通用迁移不许反向放行"
        );
        let mut c = sample();
        c.status = Performing;
        c.revert_performing_to_active().unwrap();
        assert_eq!(c.status, Active);
        // 非履行中不能调用补偿回退
        c.status = Draft;
        assert!(c.revert_performing_to_active().is_err());
    }

    fn sample() -> Contract {
        Contract {
            id: "CT-1".into(),
            project_id: "PJ-1".into(),
            contract_no: "HT-2026-001".into(),
            contract_name: "某道路工程施工".into(),
            contract_type: ContractType::Engineering,
            contract_category_code: "GC".into(),
            category_id: Some("C02".into()),
            party_a: "某建设单位".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-10".into(),
            original_amount_cent: Money::from_cent(285_000_000),
            payment_terms: "按月计量，进度款 80%".into(),
            status: ContractStatus::Draft,
            ..Default::default()
        }
    }

    /// 保存前校验要覆盖开标日期，且 Unknown 招标方式不能原样写回（会被 CHECK 拒）。
    #[test]
    fn 校验覆盖开标日期与未知招标方式() {
        let mut c = sample();
        assert!(c.validate().is_ok(), "基线样本必须合法");
        c.open_bid_date = Some("2026-13-01".into());
        assert!(c.validate().is_err(), "非法开标日期必须拒绝");
        c.open_bid_date = Some("2026-02-28".into());
        assert!(c.validate().is_ok());
        c.bid_method = Some(BidMethod::Unknown("LEGACY_X".into()));
        let err = c.validate().unwrap_err();
        assert!(err.to_string().contains("无法识别"), "{err}");
    }

    #[test]
    fn 业务字段计数口径为_31() {
        assert_eq!(business_field_count(), 31);
        assert!(BUSINESS_FIELDS.contains(&"payment_terms"));
        assert!(BUSINESS_FIELDS.contains(&"bid_method"));
        assert!(BUSINESS_FIELDS.contains(&"delivery_date"));
    }

    #[test]
    fn 合同类型只剩工程货物服务三类() {
        assert_eq!(ContractType::Supply.label(), "货物");
        assert_eq!(
            ContractType::from_code("SUPPLY"),
            Some(ContractType::Supply)
        );
        // R-57：原「施工」显示为「工程」，落库码沿用 CONSTRUCT
        assert_eq!(ContractType::default().label(), "工程");
        assert_eq!(ContractType::default().code(), "CONSTRUCT");
        assert_eq!(
            ContractType::from_code("CONSTRUCT"),
            Some(ContractType::Engineering)
        );
        // 退场的三类不再被接受
        for legacy in ["SUPERVISE", "DESIGN", "SURVEY"] {
            assert_eq!(ContractType::from_code(legacy), None, "{legacy} 应已退场");
        }
    }

    #[test]
    fn 有效计价基准以审计价完全替代() {
        let mut c = sample();
        assert!(!c.is_audited());
        assert_eq!(c.effective_base().cent(), 285_000_000);
        assert_eq!(c.audit_reduction().cent(), 0);

        c.audit_price_cent = Some(Money::from_cent(279_000_000));
        assert!(c.is_audited());
        assert_eq!(c.total_amount().cent(), 285_000_000, "合同金额只作档案");
        assert_eq!(c.effective_base().cent(), 279_000_000);
        assert_eq!(c.audit_reduction().cent(), 6_000_000);

        // 审计价为 0 / 负数按未审定处理
        c.audit_price_cent = Some(Money::ZERO);
        assert!(!c.is_audited());
        assert_eq!(c.effective_base().cent(), 285_000_000);
        c.audit_price_cent = Some(Money::from_cent(-1));
        assert!(!c.is_audited());
    }

    #[test]
    fn 状态机本期只有五态() {
        assert!(ContractStatus::Draft.can_transition_to(ContractStatus::Active));
        assert!(ContractStatus::Active.can_transition_to(ContractStatus::Performing));
        assert!(ContractStatus::Active.can_transition_to(ContractStatus::Void));
        // 审批与结算态本期不产生
        assert!(!ContractStatus::Draft.can_transition_to(ContractStatus::Pending));
        assert!(!ContractStatus::Performing.can_transition_to(ContractStatus::Settled));
        assert!(!ContractStatus::Completed.can_transition_to(ContractStatus::Closed));

        let mut c = sample();
        assert_eq!(
            c.transit(ContractStatus::Pending),
            Err(DomainError::StatusOutOfPhase("PENDING".into()))
        );
        c.transit(ContractStatus::Active).unwrap();
        assert_eq!(c.status, ContractStatus::Active);
        assert!(c.is_payable());
    }

    #[test]
    fn 保存前校验覆盖必录项与日期() {
        let mut c = sample();
        assert!(c.validate().is_ok());

        c.payment_terms = "  ".into();
        assert_eq!(c.validate(), Err(DomainError::MissingField("付款条款")));
        c.payment_terms = "x".into();

        c.contract_category_code = "C01".into();
        assert!(matches!(c.validate(), Err(DomainError::InvalidDict(_))));
        c.contract_category_code = "GC".into();

        c.planned_start = Some("2026-05-01".into());
        c.planned_end = Some("2026-04-01".into());
        assert!(matches!(c.validate(), Err(DomainError::Invariant(_))));
        c.planned_start = None;
        c.planned_end = None;

        c.sign_date = "2026-02-30".into();
        assert!(matches!(c.validate(), Err(DomainError::DateFormat(_))));
        c.sign_date = "2026-01-10".into();

        c.retention_rate_bp = 12_000;
        assert!(matches!(c.validate(), Err(DomainError::Invariant(_))));
        c.retention_rate_bp = 300;
        assert!(c.validate().is_ok());
    }

    #[test]
    fn 计划交工按含开工当日推算() {
        // 90 日历天：开工当天算第 1 天，故交工 = 开工 + 89 天
        assert_eq!(
            planned_end_for("2026-03-01", 90).as_deref(),
            Some("2026-05-29")
        );
        // 工期 1 天 → 交工日等于开工日，与 validate 的「不早于」相容
        assert_eq!(
            planned_end_for("2026-03-01", 1).as_deref(),
            Some("2026-03-01")
        );
        // 跨闰日与跨年
        assert_eq!(
            planned_end_for("2024-02-01", 29).as_deref(),
            Some("2024-02-29")
        );
        assert_eq!(
            planned_end_for("2026-12-01", 31).as_deref(),
            Some("2026-12-31")
        );
        // 任一条件不成立都不触发
        assert_eq!(planned_end_for("2026-03-01", 0), None, "工期为 0 不计算");
        assert_eq!(planned_end_for("2026-03-01", -5), None, "负工期不计算");
        assert_eq!(planned_end_for("", 90), None, "开工日为空不计算");
        assert_eq!(planned_end_for("2026-02-30", 90), None, "开工日非法不计算");
        // 推算结果必须能通过合同校验（交工不早于开工）
        let mut c = sample();
        c.planned_start = Some("2026-03-01".into());
        c.planned_end = planned_end_for("2026-03-01", 90);
        c.duration_days = 90;
        assert!(c.validate().is_ok());
    }
}
