use ctp_core::error::DomainError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("数据库错误：{0}")]
    Sql(#[from] rusqlite::Error),

    #[error("{0}")]
    Domain(#[from] DomainError),

    #[error("记录不存在：{0}")]
    NotFound(String),

    #[error("数据不一致：{0}")]
    Inconsistent(String),

    #[error("字典缺失：{0}")]
    DictMissing(String),

    #[error("报表导出失败：{0}")]
    Export(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

impl StoreError {
    /// 取领域错误引用，便于调用方区分「业务拦截」与「数据库故障」。
    pub fn domain(&self) -> Option<&DomainError> {
        match self {
            StoreError::Domain(e) => Some(e),
            _ => None,
        }
    }

    /// 是否为额度拦截（BR-01）。
    pub fn is_quota_exceeded(&self) -> bool {
        matches!(self.domain(), Some(DomainError::QuotaExceeded { .. }))
    }

    /// 是否为付清条件拦截（未交工 97% / 施工未审定或交工未满 2 年）。
    pub fn is_payment_blocked(&self) -> bool {
        matches!(self.domain(), Some(DomainError::PaymentBlocked(_)))
    }
}
