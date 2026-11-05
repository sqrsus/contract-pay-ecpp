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
}

pub type Result<T> = std::result::Result<T, DomainError>;
