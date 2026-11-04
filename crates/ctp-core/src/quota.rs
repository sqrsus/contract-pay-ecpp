use crate::contract::Contract;
use crate::error::{DomainError, Result};
use crate::money::Money;
use crate::rate::Rate;

/// 越界级别（BR-15 三支处置的判定结果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreachLevel {
    /// 未越界（`paid <= base`，等于 base 为「已付清」，不算越界）
    None,
    /// 已超基准但仍在容差内：标红放行
    WithinTolerance,
    /// 超容差：拦截
    OverTolerance,
}

impl BreachLevel {
    pub fn label(self) -> &'static str {
        match self {
            BreachLevel::None => "正常",
            BreachLevel::WithinTolerance => "已超有效计价基准（容差内）",
            BreachLevel::OverTolerance => "超容差超付",
        }
    }

    pub fn is_breached(self) -> bool {
        !matches!(self, BreachLevel::None)
    }
}

/// 可付额度（6.2.2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentQuota {
    /// 合同金额（暂定），审定后仅作档案
    pub contract_amount: Money,
    pub audit_price: Option<Money>,
    /// 有效计价基准 = 审计价 > 0 ? 审计价 : 合同金额
    pub base_amount: Money,
    pub is_audited: bool,
    /// 审减额 = 合同金额 − 审计价（派生，不落盘，可为负）
    pub audit_reduction: Money,
    /// 已付净额（仅已实付单据，作废单不计）
    pub paid_amount: Money,
    /// 在途（本期恒为 0；二期恢复申请单后按两类占用）
    pub inflight_amount: Money,
    /// 已付 + 在途
    pub occupied_amount: Money,
    /// 上限 = Base × (1 + tol)
    pub ceiling_amount: Money,
    /// 可付额度 = 上限 − 已付 − 在途（可为负，表示已超额）
    pub available_amount: Money,
    pub pay_rate: Rate,
    /// 倒挂：有效计价基准 < 累计已付
    pub is_inverted: bool,
    pub tolerance: Rate,
}

impl PaymentQuota {
    /// 纯计算，不含状态校验。`paid` 由仓储按**唯一口径**（直连 `pay_payment`）提供。
    pub fn compute(
        contract: &Contract,
        paid: Money,
        inflight: Money,
        tolerance: Rate,
    ) -> Result<Self> {
        let contract_amount = contract.total_amount();
        let audit_price = contract.audit_price();
        let base = contract.effective_base();
        let is_audited = audit_price.is_some();
        let audit_reduction = contract.audit_reduction();

        let tolerance_amt = base.mul_rate(tolerance.bp())?;
        let ceiling = base.checked_add(tolerance_amt)?;
        let occupied = paid.checked_add(inflight)?;
        let available = ceiling.checked_sub(occupied)?;

        Ok(PaymentQuota {
            contract_amount,
            audit_price,
            base_amount: base,
            is_audited,
            audit_reduction,
            paid_amount: paid,
            inflight_amount: inflight,
            occupied_amount: occupied,
            ceiling_amount: ceiling,
            available_amount: available,
            pay_rate: Rate::ratio(paid, base),
            is_inverted: paid.cent() > base.cent(),
            tolerance,
        })
    }

    /// 登记拨款前的额度快照：先校验合同可拨款，再计算。
    pub fn for_new_payment(
        contract: &Contract,
        paid: Money,
        tolerance: Rate,
    ) -> Result<Self> {
        if !contract.is_payable() {
            return Err(DomainError::ContractNotPayable(
                contract.status.code().to_string(),
            ));
        }
        Self::compute(contract, paid, Money::ZERO, tolerance)
    }

    /// BR-01 硬约束：本次金额 > 可付额度即拒绝，并给出差额。
    pub fn check_new_payment(&self, amount: Money) -> Result<()> {
        if amount.cent() <= 0 {
            return Err(DomainError::NonPositivePayment(amount.cent()));
        }
        if amount.cent() > self.available_amount.cent() {
            return Err(DomainError::QuotaExceeded {
                requested: amount.cent(),
                available: self.available_amount.cent(),
                over: amount.cent() - self.available_amount.cent(),
            });
        }
        Ok(())
    }

    /// 越界级别：标记口径**不含容差**（`base = paid` 为已付清）。
    pub fn breach_level(&self) -> BreachLevel {
        if self.paid_amount.cent() <= self.base_amount.cent() {
            BreachLevel::None
        } else if self.paid_amount.cent() <= self.ceiling_amount.cent() {
            BreachLevel::WithinTolerance
        } else {
            BreachLevel::OverTolerance
        }
    }

    /// 合同余款 = Base − 已付（可为负）。
    pub fn remaining(&self) -> Money {
        Money::from_cent(self.base_amount.cent() - self.paid_amount.cent())
    }

    /// 是否已付清（`paid = base`，不含容差）。
    pub fn is_settled(&self) -> bool {
        self.paid_amount.cent() == self.base_amount.cent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ContractStatus, ContractType};

    fn contract(amount: i64, audit: Option<i64>) -> Contract {
        Contract {
            id: "CT-1".into(),
            project_id: "PJ-1".into(),
            contract_no: "HT-2026-001".into(),
            contract_name: "测试合同".into(),
            contract_type: ContractType::Construct,
            contract_category_code: "GC".into(),
            party_a: "甲方".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-10".into(),
            original_amount_cent: Money::from_cent(amount),
            audit_price_cent: audit.map(Money::from_cent),
            payment_terms: "按月计量".into(),
            status: ContractStatus::Active,
            ..Default::default()
        }
    }

    fn yuan(v: i64) -> Money {
        Money::from_cent(v)
    }

    #[test]
    fn 额度等于基准减已付() {
        let c = contract(1_000, None);
        let q = PaymentQuota::compute(&c, yuan(300), Money::ZERO, Rate::ZERO).unwrap();
        assert_eq!(q.base_amount.cent(), 1_000);
        assert_eq!(q.available_amount.cent(), 700);
        assert_eq!(q.remaining().cent(), 700);
        assert_eq!(q.pay_rate.bp(), 3_000);
        assert_eq!(q.breach_level(), BreachLevel::None);

        // 恰好用满可以
        assert!(q.check_new_payment(yuan(700)).is_ok());
        // 多 1 分即拒绝
        assert_eq!(
            q.check_new_payment(yuan(701)),
            Err(DomainError::QuotaExceeded {
                requested: 701,
                available: 700,
                over: 1
            })
        );
        assert!(q.check_new_payment(Money::ZERO).is_err());
    }

    #[test]
    fn 审定后完全按审计价控制() {
        let c = contract(1_000, Some(900));
        let q = PaymentQuota::for_new_payment(&c, yuan(300), Rate::ZERO).unwrap();
        assert!(q.is_audited);
        assert_eq!(q.contract_amount.cent(), 1_000);
        assert_eq!(q.base_amount.cent(), 900);
        assert_eq!(q.audit_reduction.cent(), 100);
        assert_eq!(q.available_amount.cent(), 600);
        assert_eq!(q.pay_rate.bp(), 3_333);
    }

    #[test]
    fn 容差内放行超容差拦截() {
        let c = contract(1_000, None);
        // 1% 容差
        let tol = Rate::from_bp(100);
        let q = PaymentQuota::compute(&c, yuan(1_005), Money::ZERO, tol).unwrap();
        assert_eq!(q.ceiling_amount.cent(), 1_010);
        assert_eq!(q.available_amount.cent(), 5);
        assert!(q.is_inverted);
        assert_eq!(q.breach_level(), BreachLevel::WithinTolerance);
        // 容差内的 3 元可以再拨
        assert!(q.check_new_payment(yuan(5)).is_ok());
        assert!(q.check_new_payment(yuan(6)).is_err());

        let q2 = PaymentQuota::compute(&c, yuan(1_011), Money::ZERO, tol).unwrap();
        assert_eq!(q2.breach_level(), BreachLevel::OverTolerance);
        assert_eq!(q2.available_amount.cent(), -1);
        assert!(q2.check_new_payment(yuan(1)).is_err());

        // 等于基准不算越界（已付清）
        let q3 = PaymentQuota::compute(&c, yuan(1_000), Money::ZERO, tol).unwrap();
        assert!(!q3.is_inverted);
        assert!(q3.is_settled());
        assert_eq!(q3.breach_level(), BreachLevel::None);
    }

    #[test]
    fn 不可拨款状态拒绝登记() {
        let mut c = contract(1_000, None);
        c.status = ContractStatus::Draft;
        assert_eq!(
            PaymentQuota::for_new_payment(&c, Money::ZERO, Rate::ZERO),
            Err(DomainError::ContractNotPayable("DRAFT".into()))
        );
    }
}
