//! 报表数据层：屏幕表格、Excel 导出、后期打印件**共用同一份 `ReportResult`**。
//!
//! 文档 4.4.2 / FR-RPT-09 要求三者不得各自查询。本模块只做「视图行 → 列配置 + 单元格」
//! 的一次性转换：界面 `screens/reports.rs` 读它绘制表格，`xlsx.rs` 读它写工作簿，
//! 后期打印 adapter 可直接复用 `columns` / `rows` / `totals`。
//!
//! 金额在 `Cell::Money` 里恒以「分」保存（i64），只有写 Excel 单元格时才换算成「元」的
//! f64——那是 Excel 文件格式要求，不参与任何业务运算。

use ctp_core::date::now_iso;
use ctp_core::ledger::{CategoryRow, PeriodRow};
use ctp_core::money::Money;
use std::fmt;

/// 已接入导出的报表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// V3 `v_category_summary`（页面已下线，数据层保留）
    CategorySummary,
    /// V4 `v_payment_period`
    PeriodSummary,
    /// 汇总统计 · 按日
    SummaryDaily,
    /// 汇总统计 · 按项目
    SummaryProject,
    /// 汇总统计 · 按承包单位
    SummaryContractor,
}

impl ReportKind {
    pub fn code(self) -> &'static str {
        match self {
            ReportKind::CategorySummary => "CATEGORY_SUMMARY_V3",
            ReportKind::PeriodSummary => "PERIOD_SUMMARY_V4",
            ReportKind::SummaryDaily => "SUMMARY_DAILY",
            ReportKind::SummaryProject => "SUMMARY_PROJECT",
            ReportKind::SummaryContractor => "SUMMARY_CONTRACTOR",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            ReportKind::CategorySummary => "会计分类汇总",
            ReportKind::PeriodSummary => "支付期间统计",
            ReportKind::SummaryDaily => "拨款汇总 · 按日",
            ReportKind::SummaryProject => "拨款汇总 · 按项目",
            ReportKind::SummaryContractor => "拨款汇总 · 按承包单位",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            ReportKind::CategorySummary => {
                "按会计分类（ct_category）汇总，V3 口径；合同类别（工程款/材料款）不参与此处统计"
            }
            ReportKind::PeriodSummary => "按开单月份归集（V4 口径），仅统计已实付单据",
            ReportKind::SummaryDaily => "按开单日期逐日归集，仅统计已实付单据（作废单不计）",
            ReportKind::SummaryProject => "按所属项目归集，仅统计已实付单据（作废单不计）",
            ReportKind::SummaryContractor => {
                "按合同承包单位归集，仅统计已实付单据（作废单不计）"
            }
        }
    }

    pub fn file_stem(self) -> &'static str {
        match self {
            ReportKind::CategorySummary => "会计分类汇总",
            ReportKind::PeriodSummary => "支付期间统计",
            ReportKind::SummaryDaily => "拨款汇总_按日",
            ReportKind::SummaryProject => "拨款汇总_按项目",
            ReportKind::SummaryContractor => "拨款汇总_按承包单位",
        }
    }

    pub fn sheet_name(self) -> &'static str {
        match self {
            ReportKind::CategorySummary => "分类汇总",
            ReportKind::PeriodSummary => "期间统计",
            ReportKind::SummaryDaily => "按日",
            ReportKind::SummaryProject => "按项目",
            ReportKind::SummaryContractor => "按承包单位",
        }
    }
}

impl fmt::Display for ReportKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.title())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Text,
    Integer,
    /// 金额（分），写出时换算为元并套 `#,##0.00`
    Money,
    /// 比率（显示文本 + 万分比）
    Rate,
}

/// 一列的配置。屏幕与 Excel 共用 `key` / `title` / `kind`，各自只补自己的排版参数。
#[derive(Debug, Clone, PartialEq)]
pub struct ReportColumn {
    pub key: &'static str,
    pub title: &'static str,
    pub kind: ColumnKind,
    /// 数字列右对齐
    pub right: bool,
    /// Excel 列宽（字符数）
    pub width: f64,
    /// 屏幕表格该列最小宽度
    pub screen_width: f32,
    /// 屏幕表格该列吃掉剩余宽度
    pub screen_remainder: bool,
}

impl ReportColumn {
    fn new(
        key: &'static str,
        title: &'static str,
        kind: ColumnKind,
        width: f64,
        screen_width: f32,
    ) -> Self {
        ReportColumn {
            key,
            title,
            right: matches!(kind, ColumnKind::Integer | ColumnKind::Money),
            kind,
            width,
            screen_width,
            screen_remainder: false,
        }
    }
}

/// 一个单元格。导出与屏幕显示都通过 `display()` / `yuan()` 取值，不各写一套。
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    Integer(i64),
    Money(Money),
    /// （显示文本，万分比）——万分比用于进度条与 Excel 数值列，保持整数精度
    Rate(String, i32),
}

impl Cell {
    pub fn display(&self) -> String {
        match self {
            Cell::Text(s) => s.clone(),
            Cell::Integer(v) => v.to_string(),
            Cell::Money(v) => v.format_yuan(),
            Cell::Rate(s, _) => s.clone(),
        }
    }

    /// Excel 用的「元」数值；仅格式转换，不参与业务运算。
    pub fn yuan(&self) -> Option<f64> {
        match self {
            Cell::Money(v) => Some(v.cent() as f64 / 100.0),
            _ => None,
        }
    }

    pub fn integer(&self) -> Option<i64> {
        match self {
            Cell::Integer(v) => Some(*v),
            _ => None,
        }
    }

    pub fn rate_bp(&self) -> Option<i32> {
        match self {
            Cell::Rate(_, bp) => Some(*bp),
            _ => None,
        }
    }
}

/// 报表结果：屏幕 / 导出 / 打印的唯一数据源。
#[derive(Debug, Clone, PartialEq)]
pub struct ReportResult {
    pub kind: ReportKind,
    pub title: &'static str,
    /// 口径说明；汇总统计会把统计区间拼进去，所以是 String 不是 &'static str
    pub subtitle: String,
    pub unit_note: &'static str,
    pub columns: Vec<ReportColumn>,
    pub rows: Vec<Vec<Cell>>,
    pub totals: Vec<Cell>,
    pub generated_at: String,
}

impl ReportResult {
    /// V3 会计分类汇总。
    pub fn category(rows: &[CategoryRow]) -> Self {
        let mut columns = vec![
            ReportColumn::new("category_code", "分类编码", ColumnKind::Text, 14.0, 80.0),
            ReportColumn::new("category_name", "会计分类", ColumnKind::Text, 22.0, 140.0),
            ReportColumn::new("contract_count", "合同数", ColumnKind::Integer, 12.0, 70.0),
            ReportColumn::new("base_amount", "有效基准", ColumnKind::Money, 18.0, 120.0),
            ReportColumn::new("audit_amount", "审定金额", ColumnKind::Money, 18.0, 120.0),
            ReportColumn::new("paid_amount", "已付", ColumnKind::Money, 18.0, 120.0),
            ReportColumn::new("unpaid_amount", "未付", ColumnKind::Money, 18.0, 120.0),
            ReportColumn::new("pay_rate", "支付进度", ColumnKind::Rate, 16.0, 130.0),
        ];
        columns[1].screen_remainder = true;

        let data: Vec<Vec<Cell>> = rows
            .iter()
            .map(|c| {
                vec![
                    Cell::Text(c.category_code.clone()),
                    Cell::Text(c.category_name.clone()),
                    Cell::Integer(c.contract_count),
                    Cell::Money(c.base_amount),
                    Cell::Money(c.audit_amount),
                    Cell::Money(c.paid_amount),
                    Cell::Money(c.unpaid_amount),
                    Cell::Rate(c.pay_rate.percent_string(), c.pay_rate.bp()),
                ]
            })
            .collect();

        let sum = |f: fn(&CategoryRow) -> i64| -> i64 { rows.iter().map(|r| f(r)).sum() };
        let tb = sum(|r| r.base_amount.cent());
        let tp = sum(|r| r.paid_amount.cent());
        let total_bp = ratio_bp(tp, tb);
        let totals = vec![
            Cell::Text(String::new()),
            Cell::Text("合计".to_string()),
            Cell::Integer(sum(|r| r.contract_count)),
            Cell::Money(Money::from_cent(tb)),
            Cell::Money(Money::from_cent(sum(|r| r.audit_amount.cent()))),
            Cell::Money(Money::from_cent(tp)),
            Cell::Money(Money::from_cent(sum(|r| r.unpaid_amount.cent()))),
            Cell::Rate(percent_text(total_bp), total_bp),
        ];

        report(ReportKind::CategorySummary, columns, data, totals)
    }

    /// V4 支付期间统计。
    pub fn period(rows: &[PeriodRow]) -> Self {
        let mut columns = vec![
            ReportColumn::new("period_month", "期间", ColumnKind::Text, 16.0, 180.0),
            ReportColumn::new("payment_count", "单据笔数", ColumnKind::Integer, 14.0, 120.0),
            ReportColumn::new("amount", "支付金额", ColumnKind::Money, 20.0, 180.0),
            ReportColumn::new("share", "占期间合计", ColumnKind::Rate, 18.0, 160.0),
        ];
        columns[3].screen_remainder = true;

        let total: i64 = rows.iter().map(|r| r.amount.cent()).sum();
        let data: Vec<Vec<Cell>> = rows
            .iter()
            .map(|r| {
                let bp = ratio_bp(r.amount.cent(), total);
                vec![
                    Cell::Text(r.period_month.clone()),
                    Cell::Integer(r.payment_count),
                    Cell::Money(r.amount),
                    Cell::Rate(percent_text(bp), bp),
                ]
            })
            .collect();
        let total_bp = if total > 0 { 10_000 } else { 0 };
        let totals = vec![
            Cell::Text("合计".to_string()),
            Cell::Integer(rows.iter().map(|r| r.payment_count).sum()),
            Cell::Money(Money::from_cent(total)),
            Cell::Rate(percent_text(total_bp), total_bp),
        ];

        report(ReportKind::PeriodSummary, columns, data, totals)
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 汇总统计：按维度 + 日期区间归集（R-29）。
    ///
    /// 列固定为「分组名 / 单据笔数 / 支付金额 / 占合计」，三个维度共用同一套列，
    /// 只是第一列标题与数据来源不同——切换维度时合计与占比口径完全一致。
    pub fn summary(
        rows: &[ctp_core::ledger::SummaryRow],
        dim: ctp_core::ledger::SummaryDim,
        from: &str,
        to: &str,
    ) -> Self {
        use ctp_core::ledger::SummaryDim;
        let kind = match dim {
            SummaryDim::Day => ReportKind::SummaryDaily,
            SummaryDim::Project => ReportKind::SummaryProject,
            SummaryDim::Contractor => ReportKind::SummaryContractor,
        };
        let mut columns = vec![
            ReportColumn::new("group_label", dim.key_title(), ColumnKind::Text, 30.0, 200.0),
            ReportColumn::new("payment_count", "单据笔数", ColumnKind::Integer, 14.0, 110.0),
            ReportColumn::new("amount", "支付金额", ColumnKind::Money, 20.0, 170.0),
            ReportColumn::new("share", "占合计", ColumnKind::Rate, 18.0, 150.0),
        ];
        columns[0].screen_remainder = true;

        let total: i64 = rows.iter().map(|r| r.amount.cent()).sum();
        let data: Vec<Vec<Cell>> = rows
            .iter()
            .map(|r| {
                let bp = ratio_bp(r.amount.cent(), total);
                vec![
                    Cell::Text(r.label.clone()),
                    Cell::Integer(r.payment_count),
                    Cell::Money(r.amount),
                    Cell::Rate(percent_text(bp), bp),
                ]
            })
            .collect();
        let total_bp = if total > 0 { 10_000 } else { 0 };
        let totals = vec![
            Cell::Text("合计".to_string()),
            Cell::Integer(rows.iter().map(|r| r.payment_count).sum()),
            Cell::Money(Money::from_cent(total)),
            Cell::Rate(percent_text(total_bp), total_bp),
        ];

        ReportResult {
            kind,
            title: kind.title(),
            subtitle: format!("{}（统计区间 {from} ~ {to}）", kind.subtitle()),
            unit_note: "金额单位：元",
            columns,
            rows: data,
            totals,
            generated_at: now_iso(),
        }
    }

    /// 导出保存框的默认文件名：`{报表名}_{yyyyMMdd_HHmm}.xlsx`（UTC 时间戳）。
    pub fn default_file_name(&self) -> String {
        let s = &self.generated_at;
        let part = |a: usize, b: usize| s.get(a..b).unwrap_or("00");
        format!(
            "{}_{}{}{}_{}{}.xlsx",
            self.kind.file_stem(),
            part(0, 4),
            part(5, 7),
            part(8, 10),
            part(11, 13),
            part(14, 16)
        )
    }
}

fn report(
    kind: ReportKind,
    columns: Vec<ReportColumn>,
    rows: Vec<Vec<Cell>>,
    totals: Vec<Cell>,
) -> ReportResult {
    ReportResult {
        kind,
        title: kind.title(),
        subtitle: kind.subtitle().to_string(),
        unit_note: "金额单位：元",
        columns,
        rows,
        totals,
        generated_at: now_iso(),
    }
}

/// 整数万分比，避免 f64 参与业务比率运算（与 `Rate::ratio` 同口径）。
pub fn ratio_bp(num: i64, den: i64) -> i32 {
    if den == 0 {
        return 0;
    }
    let v = (num as i128) * 10_000 / (den as i128);
    v.clamp(i32::MIN as i128, i32::MAX as i128) as i32
}

pub fn percent_text(bp: i32) -> String {
    format!("{:.2}%", bp as f64 / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctp_core::rate::Rate;

    fn category_rows() -> Vec<CategoryRow> {
        vec![CategoryRow {
            project_id: "P1".into(),
            category_id: "C1".into(),
            category_code: "A01".into(),
            category_name: "建筑安装工程投资".into(),
            contract_count: 2,
            base_amount: Money::from_cent(200_000_000),
            audit_amount: Money::from_cent(0),
            paid_amount: Money::from_cent(50_000_000),
            unpaid_amount: Money::from_cent(150_000_000),
            pay_rate: Rate::from_bp(2_500),
        }]
    }

    fn period_rows() -> Vec<PeriodRow> {
        vec![
            PeriodRow {
                project_id: "P1".into(),
                period_month: "2026-06".into(),
                payment_count: 2,
                amount: Money::from_cent(30_000_000),
            },
            PeriodRow {
                project_id: "P1".into(),
                period_month: "2026-07".into(),
                payment_count: 1,
                amount: Money::from_cent(10_000_000),
            },
        ]
    }

    #[test]
    fn 分类汇总结果与合计口径一致() {
        let r = ReportResult::category(&category_rows());
        assert_eq!(r.columns.len(), 8);
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0][3].display(), "2,000,000");
        assert_eq!(r.totals[3].yuan(), Some(2_000_000.0));
        assert_eq!(r.totals[7].rate_bp(), Some(2_500));
        assert_eq!(r.totals[7].display(), "25.00%");
    }

    #[test]
    fn 期间统计占比与合计() {
        let r = ReportResult::period(&period_rows());
        assert_eq!(r.rows[0][3].rate_bp(), Some(7_500));
        assert_eq!(r.rows[1][3].display(), "25.00%");
        assert_eq!(r.totals[2].yuan(), Some(400_000.0));
        assert_eq!(r.totals[3].display(), "100.00%");
    }

    #[test]
    fn 零分母与文件名格式() {
        assert_eq!(ratio_bp(5, 0), 0);
        let r = ReportResult::period(&[]);
        assert!(r.is_empty());
        assert_eq!(r.totals[3].display(), "0.00%");
        let name = r.default_file_name();
        assert!(name.starts_with("支付期间统计_"), "{name}");
        assert!(name.ends_with(".xlsx"), "{name}");
    }
}
