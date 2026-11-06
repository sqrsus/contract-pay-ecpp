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
    /// 合同类型代码（CONSTRUCT / SUPERVISE / …），用于付清条件判定
    pub contract_type: String,
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
    /// 交工日期（项目交工登记）
    pub delivery_date: Option<String>,
}

impl ContractSummary {
    /// 项目状态口径：支付率达 100% 即视为「归档关闭」，否则「履行中」。
    pub fn is_archived(&self) -> bool {
        self.pay_rate.bp() >= 10_000
    }

    /// 是否施工类合同（付清需审定 + 交工满 2 年）。
    pub fn is_construct(&self) -> bool {
        self.contract_type.eq_ignore_ascii_case("CONSTRUCT")
    }

    /// 是否已登记交工日期。
    pub fn has_delivery(&self) -> bool {
        self.delivery_date
            .as_deref()
            .map(str::trim)
            .is_some_and(|d| !d.is_empty())
    }

    /// 视图口径的越界级别：V1 不输出容差上限，只区分「倒挂 / 正常」。
    /// 需要区分容差内与超容差时用 `PaymentQuota::breach_level`（容差来自 `sys_meta.tol_bp`）。
    pub fn breach_level(&self) -> BreachLevel {
        if self.is_inverted {
            BreachLevel::OverTolerance
        } else {
            BreachLevel::None
        }
    }

    /// 从视图行还原成可付额度快照（容差未知时按 0，仅用于展示）。
    ///
    /// 付清条件按**今天**估算（视图行不带开单日期）；登记拨款时以实际开单日期为准。
    pub fn to_quota(&self, tolerance: Rate) -> PaymentQuota {
        let pay_date = crate::date::today_iso();
        let limit = crate::quota::pay_limit_for(
            self.is_construct(),
            self.is_audited,
            self.delivery_date.as_deref(),
            self.base_amount,
            &pay_date,
        );
        let tolerance_amt = self
            .base_amount
            .mul_rate(tolerance.bp())
            .map(|m| m.cent())
            .unwrap_or(0);
        let ceiling = Money::from_cent(self.base_amount.cent() + tolerance_amt);
        let (limit_amount, cap_bp, limit_note) = match limit {
            Ok(l) if l.cap_bp >= 10_000 => (ceiling, l.cap_bp, l.note),
            Ok(l) if l.ceiling.cent() < ceiling.cent() => (l.ceiling, l.cap_bp, l.note),
            Ok(l) => (ceiling, l.cap_bp, l.note),
            Err(_) => (ceiling, 10_000, None),
        };
        let payable_amount = Money::from_cent(limit_amount.cent() - self.occupied_amount.cent());
        PaymentQuota {
            contract_amount: self.total_amount,
            audit_price: self.audit_price,
            base_amount: self.base_amount,
            is_audited: self.is_audited,
            audit_reduction: self.audit_reduction,
            paid_amount: self.paid_amount,
            inflight_amount: self.inflight_amount,
            occupied_amount: self.occupied_amount,
            ceiling_amount: ceiling,
            available_amount: self.available_amount,
            limit_amount,
            payable_amount,
            cap_bp,
            limit_note,
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

/// 汇总统计的分组维度（R-29 → R-32）。
///
/// 四个维度都只统计**已实付且未作废**的拨款单，口径与已付净额一致；
/// 按日维度已在 R-32 移除（时间粒度改由区间自选）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryDim {
    /// 按所属项目归集
    Project,
    /// 按承包单位归集
    Contractor,
    /// 按合同归集（只列出区间内有拨款的合同，R-32 确认口径）
    Contract,
    /// 按合同类型归集（施工 / 监理 / 设计 / 勘察 / 货物 / 服务）
    ContractType,
}

impl SummaryDim {
    pub fn label(self) -> &'static str {
        match self {
            SummaryDim::Project => "按项目",
            SummaryDim::Contractor => "按承包单位",
            SummaryDim::Contract => "按合同",
            SummaryDim::ContractType => "按合同类型",
        }
    }

    /// 分组列的标题（表格第一列表头）。
    pub fn key_title(self) -> &'static str {
        match self {
            SummaryDim::Project => "所属项目",
            SummaryDim::Contractor => "承包单位",
            SummaryDim::Contract => "合同",
            SummaryDim::ContractType => "合同类型",
        }
    }

    pub fn all() -> [SummaryDim; 4] {
        [
            SummaryDim::Project,
            SummaryDim::Contractor,
            SummaryDim::Contract,
            SummaryDim::ContractType,
        ]
    }
}

/// 汇总统计的筛选条件（R-32）。所有条件留空即「全量」。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SummaryFilter {
    /// 开单日期闭区间（`YYYY-MM-DD`，空串 = 不限）
    pub from: String,
    pub to: String,
    /// 项目状态：`Some(true)` 只看归档关闭（支付率 ≥ 100%），
    /// `Some(false)` 只看履行中；口径与 `ContractSummary::is_archived` 一致
    pub archived: Option<bool>,
    /// 合同类型代码（`CONSTRUCT` / `SUPERVISE` …）
    pub contract_type: Option<String>,
    /// 关键词：匹配项目名 / 承包单位 / 合同编号 / 合同名称
    pub keyword: String,
}

/// 汇总统计的一行：一个分组键的笔数与金额。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryRow {
    /// 分组键（项目 id / 承包单位 id / 合同 id / 类型代码）
    pub key: String,
    /// 显示名（项目名称 / 承包单位名称 / 合同编号·名称 / 类型中文名）
    pub label: String,
    /// 第二显示列：仅「按合同」维度填承包单位，其余维度为 None
    pub sub: Option<String>,
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
            contract_type: "CONSTRUCT".into(),
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
            delivery_date: None,
        };
        assert_eq!(s.breach_level(), BreachLevel::None);
        s.paid_amount = Money::from_cent(1_001);
        s.is_inverted = true;
        assert_eq!(s.breach_level(), BreachLevel::OverTolerance);
    }

    #[test]
    fn 项目状态按支付率自动归档() {
        let mut s = ContractSummary {
            contract_id: "C".into(),
            project_id: "P".into(),
            contract_no: "HT".into(),
            contract_name: "N".into(),
            status: "PERFORMING".into(),
            contractor_name: "O".into(),
            contract_type: "CONSTRUCT".into(),
            contract_category_code: "GC".into(),
            category_code: None,
            category_name: None,
            total_amount: Money::from_cent(1_000),
            audit_price: None,
            is_audited: false,
            base_amount: Money::from_cent(1_000),
            audit_reduction: Money::ZERO,
            paid_amount: Money::from_cent(999),
            inflight_amount: Money::ZERO,
            occupied_amount: Money::from_cent(999),
            available_amount: Money::from_cent(1),
            pay_rate: Rate::from_bp(9_990),
            is_inverted: false,
            planned_end: None,
            duration_days: 0,
            delivery_date: None,
        };
        assert!(!s.is_archived(), "未满 100% 应为履行中");
        s.pay_rate = Rate::from_bp(10_000);
        assert!(s.is_archived(), "支付率达 100% 应自动归档关闭");
    }
}
