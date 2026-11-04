use crate::date::validate_iso_date;
use crate::error::{DomainError, Result};
use crate::money::Money;

/// 支付方式（5.4.4 `pay_method`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PayMethod {
    #[default]
    Transfer,
    Acceptance,
    Cash,
    Other,
}

impl PayMethod {
    pub fn code(self) -> &'static str {
        match self {
            PayMethod::Transfer => "TRANSFER",
            PayMethod::Acceptance => "ACCEPTANCE",
            PayMethod::Cash => "CASH",
            PayMethod::Other => "OTHER",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PayMethod::Transfer => "转账",
            PayMethod::Acceptance => "承兑",
            PayMethod::Cash => "现金",
            PayMethod::Other => "其他",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "TRANSFER" => PayMethod::Transfer,
            "ACCEPTANCE" => PayMethod::Acceptance,
            "CASH" => PayMethod::Cash,
            "OTHER" => PayMethod::Other,
            _ => return None,
        })
    }
}

/// 拨款单状态。
///
/// 评审 H5 的落地结论：**本期不做红冲**。原 `REVERSAL` / `REVERSED` 两态与
/// `reversal_of` 列整体移除，错误单据用「作废 + 重登」修正——
/// `VOID` 单据不计入已付净额，效果与红冲等价，但少一个状态机和一条唯一索引。
/// 二期若恢复红冲，只在枚举与 `paid_net_subquery` 的过滤集合上扩展，其余不动。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaymentStatus {
    /// 已实付（唯一计入已付净额的状态）
    #[default]
    Paid,
    /// 作废（录错后作废重登，不计入已付净额，保留审计痕迹）
    Void,
}

impl PaymentStatus {
    pub fn code(self) -> &'static str {
        match self {
            PaymentStatus::Paid => "PAID",
            PaymentStatus::Void => "VOID",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PaymentStatus::Paid => "已付",
            PaymentStatus::Void => "作废",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "PAID" => PaymentStatus::Paid,
            "VOID" => PaymentStatus::Void,
            _ => return None,
        })
    }

    /// 是否计入「已付净额」（R-20 已付口径）。
    pub fn counts_as_paid(self) -> bool {
        matches!(self, PaymentStatus::Paid)
    }
}

/// 拨款单实体，字段与 `pay_payment` 一一对应（5.4.4）。
///
/// 与文档 DDL 的差异（均为评审结论）：
/// * 无 `application_id`：本期无申请单（H3），二期再加列，避免指向不存在的父表（M3）；
/// * 无 `reversal_of` / `reversal_reason`：本期无红冲（H5）；
/// * `amount_cent > 0`：作废替代负数红冲单，故拨款单恒为正。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Payment {
    pub id: String,
    pub payment_no: String,
    pub title: Option<String>,
    pub sort_no: i64,
    pub period_no: i64,
    pub contract_id: String,
    pub project_id: String,
    pub pay_date: String,
    pub amount_cent: Money,
    pub reason: Option<String>,
    pub pay_method: PayMethod,
    pub bank_serial_no: Option<String>,
    pub pay_account: Option<String>,
    pub receive_account: Option<String>,
    pub status: PaymentStatus,
    pub operator_id: String,
    pub print_count: i64,
    pub remark: Option<String>,
    pub is_deleted: bool,
    pub version: i64,
    pub created_at: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

impl Payment {
    /// 计入「已付净额」的判定：已实付且未逻辑删除。
    pub fn counts_as_paid(&self) -> bool {
        self.status.counts_as_paid() && !self.is_deleted
    }

    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(DomainError::MissingField("id"));
        }
        if self.payment_no.trim().is_empty() {
            return Err(DomainError::MissingField("拨款单号"));
        }
        if self.contract_id.trim().is_empty() {
            return Err(DomainError::MissingField("合同"));
        }
        if self.project_id.trim().is_empty() {
            return Err(DomainError::MissingField("项目"));
        }
        if self.operator_id.trim().is_empty() {
            return Err(DomainError::MissingField("经办人"));
        }
        if self.amount_cent.cent() <= 0 {
            return Err(DomainError::NonPositivePayment(self.amount_cent.cent()));
        }
        validate_iso_date(&self.pay_date)?;
        if self.period_no < 0 {
            return Err(DomainError::Invariant("期数不能为负".into()));
        }
        Ok(())
    }

    /// 作废：仅已付单可作废，作废后不计入已付净额。
    pub fn void(&mut self) -> Result<()> {
        if self.status != PaymentStatus::Paid {
            return Err(DomainError::IllegalState(format!(
                "拨款单 {} 当前为 {}，不可重复作废",
                self.payment_no,
                self.status.label()
            )));
        }
        self.status = PaymentStatus::Void;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Payment {
        Payment {
            id: "PM-1".into(),
            payment_no: "BK-2026-0001".into(),
            contract_id: "CT-1".into(),
            project_id: "PJ-1".into(),
            pay_date: "2026-06-30".into(),
            amount_cent: Money::from_cent(1_000_000),
            operator_id: "U-1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn 拨款单金额必须为正() {
        let mut p = sample();
        assert!(p.validate().is_ok());
        p.amount_cent = Money::ZERO;
        assert_eq!(p.validate(), Err(DomainError::NonPositivePayment(0)));
        p.amount_cent = Money::from_cent(-100);
        assert_eq!(p.validate(), Err(DomainError::NonPositivePayment(-100)));
    }

    #[test]
    fn 作废后不再计入已付净额() {
        let mut p = sample();
        assert!(p.counts_as_paid());
        p.void().unwrap();
        assert!(!p.counts_as_paid());
        assert_eq!(p.status.code(), "VOID");
        assert!(p.void().is_err(), "不可重复作废");
    }
}
