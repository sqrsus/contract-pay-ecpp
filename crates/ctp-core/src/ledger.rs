use crate::money::Money;
use crate::quota::{BreachLevel, PaymentQuota};
use crate::rate::Rate;

/// `v_contract_summary`（V1）的一行：报表与界面的唯一汇总口径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractSummary {
    pub contract_id: String,
    pub project_id: String,
    pub contract_no: String,
    pub contract_name: String,
    pub status: String,
    pub contractor_name: String,
    pub contract_category_code: String,
    pub category_code: Option<String>,
    pub category_name: Option<String>,
    pub total_amount: Money,
    pub audit_price: Option<Money>,
    pub is_audited: bool,
    pub base_amount: Money,
    pub audit_reduction: Money,
    pub paid_amount: Money,
    pub inflight_amount: Money,
    pub occupied_amount: Money,
    pub available_amount: Money,
    pub pay_rate: Rate,
    pub is_inverted: bool,
    pub planned_end: Option<String>,
    pub duration_days: i64,
}

impl ContractSummary {
    /// 视图口径的越界级别：V1 不输出容差上限，只区分「倒挂 / 正常」。
    /// 需要区分容差内与超容差时用 `PaymentQuota::breach_level`（容差来自 `sys_meta.tol_bp`）。
    pub fn breach_level(&self) -> BreachLevel {
        if self.is_inverted {
            BreachLevel::OverTolerance
        } else {
            BreachLevel::None
        }
    }

    /// 从视图行还原成本可付额度快照（容差未知时按 0，仅用于展示）。
    pub fn to_quota(&self, tolerance: Rate) -> PaymentQuota {
        PaymentQuota {
            contract_amount: self.total_amount,
            audit_price: self.audit_price,
            base_amount: self.base_amount,
            is_audited: self.is_audited,
            audit_reduction: self.audit_reduction,
            paid_amount: self.paid_amount,
            inflight_amount: self.inflight_amount,
            occupied_amount: self.occupied_amount,
            ceiling_amount: self.base_amount,
            available_amount: self.available_amount,
            pay_rate: self.pay_rate,
            is_inverted: self.is_inverted,
            tolerance,
        }
    }
}

/// `v_payment_ledger`（V2）的一行：拨款一览表（RPT-01）数据源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRow {
    pub pay_date: String,
    pub payment_no: String,
    pub payment_status: String,
    pub project_name: String,
    pub contract_no: String,
    pub contract_name: String,
    pub contractor_name: String,
    pub category_name: Option<String>,
    pub reason: Option<String>,
    pub amount: Money,
    /// 同一合同按 (pay_date, payment_no) 的累计已付
    pub cum_amount: Money,
    pub total_amount: Money,
    pub audit_price: Option<Money>,
    pub base_amount: Money,
    pub is_audited: bool,
    pub pay_rate: Rate,
}

/// `v_category_summary`（V3）的一行：合同会计分类汇总（RPT-02）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryRow {
    pub project_id: String,
    pub category_id: String,
    pub category_code: String,
    pub category_name: String,
    pub contract_count: i64,
    pub base_amount: Money,
    pub audit_amount: Money,
    pub paid_amount: Money,
    pub unpaid_amount: Money,
    pub pay_rate: Rate,
}

/// `v_payment_period`（V4）的一行：支付期间统计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodRow {
    pub project_id: String,
    pub period_month: String,
    pub payment_count: i64,
    pub amount: Money,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 汇总行越界级别不含容差() {
        let mut s = ContractSummary {
            contract_id: "C".into(),
            project_id: "P".into(),
            contract_no: "HT".into(),
            contract_name: "N".into(),
            status: "ACTIVE".into(),
            contractor_name: "O".into(),
            contract_category_code: "GC".into(),
            category_code: None,
            category_name: None,
            total_amount: Money::from_cent(1_000),
            audit_price: None,
            is_audited: false,
            base_amount: Money::from_cent(1_000),
            audit_reduction: Money::ZERO,
            paid_amount: Money::from_cent(900),
            inflight_amount: Money::ZERO,
            occupied_amount: Money::from_cent(900),
            available_amount: Money::from_cent(100),
            pay_rate: Rate::from_bp(9_000),
            is_inverted: false,
            planned_end: None,
            duration_days: 0,
        };
        assert_eq!(s.breach_level(), BreachLevel::None);
        s.paid_amount = Money::from_cent(1_001);
        s.is_inverted = true;
        assert_eq!(s.breach_level(), BreachLevel::OverTolerance);
    }
}
