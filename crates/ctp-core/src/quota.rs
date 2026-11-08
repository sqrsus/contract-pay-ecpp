use crate::contract::{Contract, ContractType};
use crate::date;
use crate::error::{DomainError, Result};
use crate::money::Money;
use crate::rate::Rate;

/// 未满足「付清」条件时的累计付款上限（有效计价基准的 80%）。
///
/// 该上限是硬约束：不叠加容差。口径见 `pay_limit`。
pub const UNSETTLED_CAP_BP: i32 = 8_000;

/// 施工合同需要「交工日期距开单日期严格大于该年数」才能付清。
pub const SETTLE_YEARS: i32 = 2;

/// 付清条件快照：本期最多能把「累计已付」推到多少。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayLimit {
    /// 允许的累计已付上限（不含容差）
    pub ceiling: Money,
    /// 上限比例：8000 = 80%，10000 = 100%
    pub cap_bp: i32,
    /// 受限原因；`None` 表示无额外限制，可按「基准 + 容差」付款
    pub note: Option<&'static str>,
}

impl PayLimit {
    pub fn is_restricted(&self) -> bool {
        self.note.is_some()
    }
}

/// 按「付清条件」计算累计付款上限。
///
/// 两条规则（业务方 2026-11-05 确认）：
/// 1. **所有未交工合同**：累计付款不得超过有效计价基准的 80%；
/// 2. **施工合同**：须已登记审计价、且交工日期距本次开单日期**严格超过 2 个自然年**，
///    才允许付到 100%；否则同样按 80% 封顶。
///
/// 非施工合同只要登记了交工日期即可付清。
pub fn pay_limit(contract: &Contract, base: Money, pay_date: &str) -> Result<PayLimit> {
    pay_limit_for(
        contract.contract_type == ContractType::Construct,
        contract.is_audited(),
        contract.delivery_date.as_deref(),
        base,
        pay_date,
    )
}

/// 同 `pay_limit`，但不依赖 `Contract`——供视图行（`ContractSummary`）复用同一套口径。
pub fn pay_limit_for(
    is_construct: bool,
    is_audited: bool,
    delivery_date: Option<&str>,
    base: Money,
    pay_date: &str,
) -> Result<PayLimit> {
    let capped = |note: &'static str| -> Result<PayLimit> {
        Ok(PayLimit {
            ceiling: base.mul_rate(UNSETTLED_CAP_BP)?,
            cap_bp: UNSETTLED_CAP_BP,
            note: Some(note),
        })
    };

    let delivery = delivery_date.map(str::trim).filter(|d| !d.is_empty());
    let Some(delivery) = delivery else {
        return capped("未登记交工日期，累计付款不得超过有效计价基准的 80%");
    };

    if is_construct {
        if !is_audited {
            return capped("施工合同未登记审计价，累计付款不得超过有效计价基准的 80%");
        }
        if !date::years_elapsed_over(delivery, pay_date, SETTLE_YEARS) {
            return capped("施工合同交工未满 2 年，累计付款不得超过有效计价基准的 80%");
        }
    }

    Ok(PayLimit {
        ceiling: base,
        cap_bp: 10_000,
        note: None,
    })
}

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
            BreachLevel::WithinTolerance => "已超有效计价基准",
            BreachLevel::OverTolerance => "超付拦截",
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
    ///
    /// 这是**视图口径**：只受「基准 + 容差」约束，不含付清条件限制。
    /// 登记拨款请用 `payable_amount`。
    pub available_amount: Money,
    /// 付清条件允许的累计上限（= min(ceiling, 80% 封顶)）
    pub limit_amount: Money,
    /// 本期实际可付 = `limit_amount − 已付 − 在途`（可为负）
    pub payable_amount: Money,
    /// 封顶比例：8000 = 80%，10000 = 100%
    pub cap_bp: i32,
    /// 付清条件受限原因；`None` 表示无额外限制
    pub limit_note: Option<&'static str>,
    pub pay_rate: Rate,
    /// 倒挂：有效计价基准 < 累计已付
    pub is_inverted: bool,
    pub tolerance: Rate,
}

impl PaymentQuota {
    /// 纯计算，不含状态校验。`paid` 由仓储按**唯一口径**（直连 `pay_payment`）提供。
    ///
    /// `pay_date` 是本次开单日期，用于判定「交工满 2 年」。
    pub fn compute(
        contract: &Contract,
        paid: Money,
        inflight: Money,
        tolerance: Rate,
        pay_date: &str,
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

        let limit = pay_limit(contract, base, pay_date)?;
        // 受限时按 80% 硬封顶（不叠加容差）；不受限时沿用「基准 + 容差」口径。
        let limit_amount = if limit.cap_bp >= 10_000 {
            ceiling
        } else if limit.ceiling.cent() < ceiling.cent() {
            limit.ceiling
        } else {
            ceiling
        };
        let payable = limit_amount.checked_sub(occupied)?;

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
            limit_amount,
            payable_amount: payable,
            cap_bp: limit.cap_bp,
            limit_note: limit.note,
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
        pay_date: &str,
    ) -> Result<Self> {
        if !contract.is_payable() {
            return Err(DomainError::ContractNotPayable(
                contract.status.code().to_string(),
            ));
        }
        Self::compute(contract, paid, Money::ZERO, tolerance, pay_date)
    }

    /// BR-01 硬约束：本次金额 > 可付额度即拒绝，并给出差额。
    ///
    /// 可付额度取 `payable_amount`——已叠加付清条件封顶。
    /// 因封顶被拒时返回 `PaymentBlocked`，提示里带上限原因。
    pub fn check_new_payment(&self, amount: Money) -> Result<()> {
        if amount.cent() <= 0 {
            return Err(DomainError::NonPositivePayment(amount.cent()));
        }
        if amount.cent() > self.payable_amount.cent() {
            if let Some(note) = self.limit_note {
                return Err(DomainError::PaymentBlocked(format!(
                    "{note}；本次最多可付 {} 元",
                    Money::from_cent(self.payable_amount.cent().max(0)).format_yuan()
                )));
            }
            return Err(DomainError::QuotaExceeded {
                requested: amount.cent(),
                available: self.payable_amount.cent(),
                over: amount.checked_sub(self.payable_amount)?.cent(),
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
        self.base_amount.saturating_sub(self.paid_amount)
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

    /// 默认合同：非施工 + 已登记交工日期，不受付清条件封顶。
    /// 这样既有用例继续验证「基准 + 容差」的原始额度口径。
    fn contract(amount: i64, audit: Option<i64>) -> Contract {
        Contract {
            id: "CT-1".into(),
            project_id: "PJ-1".into(),
            contract_no: "HT-2026-001".into(),
            contract_name: "测试合同".into(),
            contract_type: ContractType::Supervise,
            contract_category_code: "FW".into(),
            party_a: "甲方".into(),
            contractor_id: "ORG-1".into(),
            sign_date: "2026-01-10".into(),
            original_amount_cent: Money::from_cent(amount),
            audit_price_cent: audit.map(Money::from_cent),
            payment_terms: "按月计量".into(),
            status: ContractStatus::Active,
            delivery_date: Some("2020-01-01".into()),
            ..Default::default()
        }
    }

    /// 施工合同：显式控制审计价与交工日期。
    fn construct(amount: i64, audit: Option<i64>, delivery: Option<&str>) -> Contract {
        Contract {
            contract_type: ContractType::Construct,
            contract_category_code: "GC".into(),
            delivery_date: delivery.map(str::to_string),
            ..contract(amount, audit)
        }
    }

    /// 测试用开单日期。
    const PAY_DATE: &str = "2026-03-31";

    fn yuan(v: i64) -> Money {
        Money::from_cent(v)
    }

    #[test]
    fn 额度等于基准减已付() {
        let c = contract(1_000, None);
        let q = PaymentQuota::compute(&c, yuan(300), Money::ZERO, Rate::ZERO, PAY_DATE).unwrap();
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
        let q = PaymentQuota::for_new_payment(&c, yuan(300), Rate::ZERO, PAY_DATE).unwrap();
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
        let q = PaymentQuota::compute(&c, yuan(1_005), Money::ZERO, tol, PAY_DATE).unwrap();
        assert_eq!(q.ceiling_amount.cent(), 1_010);
        assert_eq!(q.available_amount.cent(), 5);
        assert!(q.is_inverted);
        assert_eq!(q.breach_level(), BreachLevel::WithinTolerance);
        // 容差内的 5 元可以再拨
        assert!(q.check_new_payment(yuan(5)).is_ok());
        assert!(q.check_new_payment(yuan(6)).is_err());

        let q2 = PaymentQuota::compute(&c, yuan(1_011), Money::ZERO, tol, PAY_DATE).unwrap();
        assert_eq!(q2.breach_level(), BreachLevel::OverTolerance);
        assert_eq!(q2.available_amount.cent(), -1);
        assert!(q2.check_new_payment(yuan(1)).is_err());

        // 等于基准不算越界（已付清）
        let q3 = PaymentQuota::compute(&c, yuan(1_000), Money::ZERO, tol, PAY_DATE).unwrap();
        assert!(!q3.is_inverted);
        assert!(q3.is_settled());
        assert_eq!(q3.breach_level(), BreachLevel::None);
    }

    #[test]
    fn 不可拨款状态拒绝登记() {
        let mut c = contract(1_000, None);
        c.status = ContractStatus::Draft;
        assert_eq!(
            PaymentQuota::for_new_payment(&c, Money::ZERO, Rate::ZERO, PAY_DATE),
            Err(DomainError::ContractNotPayable("DRAFT".into()))
        );
    }

    #[test]
    fn 未交工一律封顶八成() {
        let c = construct(1_000, Some(900), None);
        let q = PaymentQuota::for_new_payment(&c, yuan(300), Rate::ZERO, PAY_DATE).unwrap();
        assert_eq!(q.cap_bp, UNSETTLED_CAP_BP);
        assert_eq!(q.available_amount.cent(), 600, "视图口径仍是基准减已付");
        assert_eq!(q.payable_amount.cent(), 420, "900 × 80% − 300");
        assert!(q.limit_note.unwrap().contains("未登记交工日期"));

        assert!(q.check_new_payment(yuan(420)).is_ok(), "恰好用满 80% 可以");
        match q.check_new_payment(yuan(421)) {
            Err(DomainError::PaymentBlocked(msg)) => {
                assert!(msg.contains("80%"), "提示应说明 80% 封顶：{msg}");
            }
            other => panic!("应被付清条件拦截，实得 {other:?}"),
        }
    }

    #[test]
    fn 非施工合同交工后即可付清() {
        let c = contract(1_000, None);
        let q = PaymentQuota::for_new_payment(&c, yuan(300), Rate::ZERO, PAY_DATE).unwrap();
        assert_eq!(q.cap_bp, 10_000);
        assert!(q.limit_note.is_none());
        assert_eq!(q.payable_amount.cent(), 700);
        assert!(q.check_new_payment(yuan(700)).is_ok());
    }

    #[test]
    fn 施工合同付清需审定且交工满两年() {
        // 未审定 → 80%
        let c = construct(1_000, None, Some("2020-01-01"));
        let q = PaymentQuota::compute(&c, Money::ZERO, Money::ZERO, Rate::ZERO, PAY_DATE).unwrap();
        assert_eq!(q.cap_bp, UNSETTLED_CAP_BP);
        assert!(q.limit_note.unwrap().contains("未登记审计价"));

        // 已审定但交工未满 2 年 → 80%
        let c = construct(1_000, Some(1_000), Some("2024-06-01"));
        let q =
            PaymentQuota::compute(&c, Money::ZERO, Money::ZERO, Rate::ZERO, "2026-05-31").unwrap();
        assert_eq!(q.cap_bp, UNSETTLED_CAP_BP);
        assert!(q.limit_note.unwrap().contains("未满 2 年"));

        // 恰好满 2 年（同日）不算「大于 2 年」
        let q =
            PaymentQuota::compute(&c, Money::ZERO, Money::ZERO, Rate::ZERO, "2026-06-01").unwrap();
        assert_eq!(q.cap_bp, UNSETTLED_CAP_BP);

        // 严格超过 2 年 → 可付清
        let q =
            PaymentQuota::compute(&c, Money::ZERO, Money::ZERO, Rate::ZERO, "2026-06-02").unwrap();
        assert_eq!(q.cap_bp, 10_000);
        assert!(q.limit_note.is_none());
        assert_eq!(q.payable_amount.cent(), 1_000);
    }
}
