use thiserror::Error;

/// 领域错误。所有金额/状态类错误都必须走这里，禁止用字符串裸抛。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    #[error("金额溢出")]
    MoneyOverflow,

    #[error("金额格式非法：{0}")]
    MoneyFormat(String),

    #[error("日期格式非法（应 YYYY-MM-DD）：{0}")]
    DateFormat(String),

    #[error("金额不能为负：{0} 分")]
    NegativeAmount(i64),

    /// 编辑合同金额时低于「累计已付」或「审计价」。
    ///
    /// 低于累计已付会让支付率倒挂（已付 > 基准），该合同此后算出的可付额度为负，
    /// 既不能登记拨款、也无法通过正常编辑回到可用状态；低于审计价则与审定结论冲突。
    /// 单独成变体，界面据此提示差额而不是一句泛泛的「保存失败」。
    #[error("金额低于下限：新金额 {requested} 分，下限 {floor} 分（取累计已付与审计价的较大者）")]
    AmountBelowFloor { requested: i64, floor: i64 },

    #[error("必备字段缺失：{0}")]
    MissingField(&'static str),

    #[error("字典值非法：{0}")]
    InvalidDict(String),

    #[error("合同状态 {0} 本期不产生（审批/结算模块已移出本期范围）")]
    StatusOutOfPhase(String),

    #[error("合同状态 {0} 不允许登记拨款")]
    ContractNotPayable(String),

    #[error("合同已有拨款记录，不允许作废")]
    ContractHasPayments,

    #[error("拨款金额必须大于 0，实际 {0} 分")]
    NonPositivePayment(i64),

    #[error("超出可付额度：本次 {requested} 分，可用 {available} 分，超出 {over} 分")]
    QuotaExceeded {
        requested: i64,
        available: i64,
        over: i64,
    },

    #[error("付款受付清条件限制：{0}")]
    PaymentBlocked(String),

    #[error("单据状态 {0} 不允许该操作")]
    IllegalState(String),

    #[error("合同附件缺失：{0}（合同正本为必传项，补齐后才能生效）")]
    AttachmentRequired(&'static str),

    #[error("口径违例：{0}")]
    Invariant(String),

    /// 提交内容与库里的值逐字段相同：不是口径问题，也不需要留痕（R-51）。
    ///
    /// 单独成变体是为了让界面提示不再顶「口径违例」的帽子——用户看到的应是
    /// 「没有修改任何内容，无需留痕」，而不是一句像出错的诊断。
    #[error("{0}")]
    NoChange(String),

    /// 操作本身合法，但会改变已有单据的适用规则，需要用户二次确认。
    ///
    /// 目前只有一处：合同类型从工程改为货物 / 服务（或反向），会改变付清条件
    /// （工程类受 80% 封顶 + 交工满 2 年限制），原本合规的历史付款可能瞬间「超付」。
    /// 界面收到该错误后弹确认框，确认后带 `confirmed = true` 重新提交。
    #[error("{0}")]
    NeedConfirm(String),
}

pub type Result<T> = std::result::Result<T, DomainError>;
