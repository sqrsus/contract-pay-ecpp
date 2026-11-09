//! 报表数据层：屏幕表格、Excel 导出、后期打印件**共用同一份 `ReportResult`**。
//!
//! 文档 4.4.2 / FR-RPT-09 要求三者不得各自查询。本模块只做「视图行 → 列配置 + 单元格」
//! 的一次性转换：界面 `screens/reports.rs` 读它绘制表格，`xlsx.rs` 读它写工作簿，
//! 后期打印 adapter 可直接复用 `columns` / `rows` / `totals`。
//!
//! 金额在 `Cell::Money` 里恒以「分」保存（i64），只有写 Excel 单元格时才换算成「元」的
//! f64——那是 Excel 文件格式要求，不参与任何业务运算。

use ctp_core::date::now_iso;
use ctp_core::ledger::{CategoryRow, ContractSummary, PeriodRow};
use ctp_core::money::Money;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// 已接入导出的报表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// V3 `v_category_summary`（页面已下线，数据层保留）
    CategorySummary,
    /// V4 `v_payment_period`
    PeriodSummary,
    /// 汇总统计 · 按项目
    SummaryProject,
    /// 汇总统计 · 按承包单位
    SummaryContractor,
    /// 汇总统计 · 按合同
    SummaryContract,
    /// 汇总统计 · 按合同类型
    SummaryContractType,
    /// 合同台账（数据源 V1 `v_contract_summary`）
    ContractLedger,
    /// 拨款一览（数据源 `pay_payment`）
    PaymentLedger,
}

impl ReportKind {
    pub fn code(self) -> &'static str {
        match self {
            ReportKind::CategorySummary => "CATEGORY_SUMMARY_V3",
            ReportKind::PeriodSummary => "PERIOD_SUMMARY_V4",
            ReportKind::SummaryProject => "SUMMARY_PROJECT",
            ReportKind::SummaryContractor => "SUMMARY_CONTRACTOR",
            ReportKind::SummaryContract => "SUMMARY_CONTRACT",
            ReportKind::SummaryContractType => "SUMMARY_CONTRACT_TYPE",
            ReportKind::ContractLedger => "CONTRACT_LEDGER_V1",
            ReportKind::PaymentLedger => "PAYMENT_LEDGER_V1",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            ReportKind::CategorySummary => "会计分类汇总",
            ReportKind::PeriodSummary => "支付期间统计",
            ReportKind::SummaryProject => "拨款汇总 · 按项目",
            ReportKind::SummaryContractor => "拨款汇总 · 按承包单位",
            ReportKind::SummaryContract => "拨款汇总 · 按合同",
            ReportKind::SummaryContractType => "拨款汇总 · 按合同类型",
            ReportKind::ContractLedger => "合同台账",
            ReportKind::PaymentLedger => "拨款一览",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            ReportKind::CategorySummary => {
                "按会计分类（ct_category）汇总，V3 口径；合同类别（工程款/材料款）不参与此处统计"
            }
            ReportKind::PeriodSummary => "按开单月份归集（V4 口径），仅统计已实付单据",
            ReportKind::SummaryProject => "按所属项目归集，仅统计已实付单据（作废单不计）",
            ReportKind::SummaryContractor => "按合同承包单位归集，仅统计已实付单据（作废单不计）",
            ReportKind::SummaryContract => {
                "按合同归集（只列区间内有拨款的合同），仅统计已实付单据（作废单不计）"
            }
            ReportKind::SummaryContractType => {
                "按合同类型归集（施工/监理/设计/勘察/货物/服务），仅统计已实付单据"
            }
            ReportKind::ContractLedger => "按合同汇总金额与支付进度（数据源 V1 视图）",
            ReportKind::PaymentLedger => "按合同列出拨款单，作废单保留记录、不计入合计",
        }
    }

    pub fn file_stem(self) -> &'static str {
        match self {
            ReportKind::CategorySummary => "会计分类汇总",
            ReportKind::PeriodSummary => "支付期间统计",
            ReportKind::SummaryProject => "拨款汇总_按项目",
            ReportKind::SummaryContractor => "拨款汇总_按承包单位",
            ReportKind::SummaryContract => "拨款汇总_按合同",
            ReportKind::SummaryContractType => "拨款汇总_按合同类型",
            ReportKind::ContractLedger => "合同台账",
            ReportKind::PaymentLedger => "拨款一览",
        }
    }

    pub fn sheet_name(self) -> &'static str {
        match self {
            ReportKind::CategorySummary => "分类汇总",
            ReportKind::PeriodSummary => "期间统计",
            ReportKind::SummaryProject => "按项目",
            ReportKind::SummaryContractor => "按承包单位",
            ReportKind::SummaryContract => "按合同",
            ReportKind::SummaryContractType => "按合同类型",
            ReportKind::ContractLedger => "合同台账",
            ReportKind::PaymentLedger => "拨款一览",
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

    /// Excel 表头的实际文字：金额列统一补「（元）」。
    ///
    /// 屏幕表头与导出列选择列表也读这里，保证「选择的列名」和「Excel 表头」逐字一致，
    /// 不再把「（元）」的拼接逻辑散落在写文件的地方。
    pub fn excel_title(&self) -> String {
        if self.kind == ColumnKind::Money {
            format!("{}（元）", self.title)
        } else {
            self.title.to_string()
        }
    }
}

/// 列裁剪 / 形状校验的错误。只在导出前构造结果时产生，不碰业务数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportColumnError {
    NoColumns,
    EmptySelection,
    Duplicate(String),
    Unknown(String),
    RowShape {
        row: usize,
        actual: usize,
        expected: usize,
    },
    TotalsShape {
        actual: usize,
        expected: usize,
    },
}

impl fmt::Display for ReportColumnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReportColumnError::NoColumns => f.write_str("报表没有任何可导出的列"),
            ReportColumnError::EmptySelection => f.write_str("未选择任何导出列，请至少选择一列"),
            ReportColumnError::Duplicate(k) => write!(f, "导出列重复：{k}"),
            ReportColumnError::Unknown(k) => write!(f, "导出列不在当前字段中：{k}"),
            ReportColumnError::RowShape {
                row,
                actual,
                expected,
            } => write!(f, "第 {} 行有 {actual} 个单元格，应为 {expected}", row + 1),
            ReportColumnError::TotalsShape { actual, expected } => {
                write!(f, "合计行有 {actual} 个单元格，应为 {expected}")
            }
        }
    }
}

impl std::error::Error for ReportColumnError {}

/// 拨款一览导出用的一份合同字段（按 `contract_id` 关联）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaymentContractFields {
    pub contract_name: String,
    pub contractor_name: String,
}

/// `contract_id → 合同字段`。查不到就留空，不阻断导出。
pub type PaymentContractLookup = BTreeMap<String, PaymentContractFields>;

/// 从 V1 汇总行构造查找表；同一合同 id 只保留首条，避免重复行影响取值。
pub fn payment_contract_lookup(summaries: &[ContractSummary]) -> PaymentContractLookup {
    let mut out = PaymentContractLookup::new();
    for s in summaries {
        out.entry(s.contract_id.clone())
            .or_insert_with(|| PaymentContractFields {
                contract_name: s.contract_name.clone(),
                contractor_name: s.contractor_name.clone(),
            });
    }
    out
}

/// 合同台账的可见列（R-33）：窄屏时按宽度逐级降列，与页面原有 9→7→5 列策略一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerColumns {
    /// 承包单位 + 合同金额
    pub wide: bool,
    /// 审计价 + 交工日期
    pub medium: bool,
}

impl LedgerColumns {
    pub const FULL: Self = LedgerColumns {
        wide: true,
        medium: true,
    };
    pub const COMPACT: Self = LedgerColumns {
        wide: false,
        medium: true,
    };
    pub const TIGHT: Self = LedgerColumns {
        wide: false,
        medium: false,
    };
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

        let sum =
            |f: fn(&CategoryRow) -> i64| -> i64 { Money::sum_saturating(rows.iter().map(&f)) };
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
            ReportColumn::new(
                "payment_count",
                "单据笔数",
                ColumnKind::Integer,
                14.0,
                120.0,
            ),
            ReportColumn::new("amount", "支付金额", ColumnKind::Money, 20.0, 180.0),
            ReportColumn::new("share", "占期间合计", ColumnKind::Rate, 18.0, 160.0),
        ];
        columns[3].screen_remainder = true;

        let total: i64 = Money::sum_saturating(rows.iter().map(|r| r.amount.cent()));
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
            Cell::Integer(Money::sum_saturating(rows.iter().map(|r| r.payment_count))),
            Cell::Money(Money::from_cent(total)),
            Cell::Rate(percent_text(total_bp), total_bp),
        ];

        report(ReportKind::PeriodSummary, columns, data, totals)
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 形状校验：列 / 数据行 / 合计行三者长度必须一致，且至少有一列。
    ///
    /// 写 Excel 时按 `columns[ci]` 取列语义，若行比列短就会越界；裁剪或追加列之后
    /// 统一在这里挡一次，把「静默 panic」变成可展示的导出错误。
    pub fn validate_shape(&self) -> Result<(), ReportColumnError> {
        let n = self.columns.len();
        if n == 0 {
            return Err(ReportColumnError::NoColumns);
        }
        for (i, row) in self.rows.iter().enumerate() {
            if row.len() != n {
                return Err(ReportColumnError::RowShape {
                    row: i,
                    actual: row.len(),
                    expected: n,
                });
            }
        }
        if self.totals.len() != n {
            return Err(ReportColumnError::TotalsShape {
                actual: self.totals.len(),
                expected: n,
            });
        }
        Ok(())
    }

    /// 按用户选择的列 key **顺序**裁剪 / 重排一份新结果（R-46 合同台账导出选列）。
    ///
    /// 列名、列宽、单元格类型、数据行、合计行都从原结果按同一组下标取，
    /// 因此 Excel 只写所选字段、列名与顺序与选择一致，且合计 / 金额格式不会错列。
    pub fn select_columns(&self, keys: &[&str]) -> Result<Self, ReportColumnError> {
        self.validate_shape()?;
        if keys.is_empty() {
            return Err(ReportColumnError::EmptySelection);
        }
        let mut seen = BTreeSet::new();
        let mut indices = Vec::with_capacity(keys.len());
        for key in keys {
            if !seen.insert(*key) {
                return Err(ReportColumnError::Duplicate((*key).to_string()));
            }
            let idx = self
                .columns
                .iter()
                .position(|c| c.key == *key)
                .ok_or_else(|| ReportColumnError::Unknown((*key).to_string()))?;
            indices.push(idx);
        }
        let columns: Vec<ReportColumn> = indices.iter().map(|&i| self.columns[i].clone()).collect();
        let rows: Vec<Vec<Cell>> = self
            .rows
            .iter()
            .map(|row| indices.iter().map(|&i| row[i].clone()).collect())
            .collect();
        let totals: Vec<Cell> = indices.iter().map(|&i| self.totals[i].clone()).collect();
        Ok(ReportResult {
            kind: self.kind,
            title: self.title,
            subtitle: self.subtitle.clone(),
            unit_note: self.unit_note,
            columns,
            rows,
            totals,
            generated_at: self.generated_at.clone(),
        })
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
        filter_note: &str,
    ) -> Self {
        use ctp_core::contract::ContractType;
        use ctp_core::ledger::SummaryDim;
        let kind = match dim {
            SummaryDim::Project => ReportKind::SummaryProject,
            SummaryDim::Contractor => ReportKind::SummaryContractor,
            SummaryDim::Contract => ReportKind::SummaryContract,
            SummaryDim::ContractType => ReportKind::SummaryContractType,
        };
        // 「按合同」多一列承包单位（R-32 确认口径：合同行信息量大，单独给 5 列）
        let mut columns = if dim == SummaryDim::Contract {
            vec![
                ReportColumn::new(
                    "group_label",
                    dim.key_title(),
                    ColumnKind::Text,
                    30.0,
                    220.0,
                ),
                ReportColumn::new("sub", "承包单位", ColumnKind::Text, 20.0, 170.0),
                ReportColumn::new(
                    "payment_count",
                    "单据笔数",
                    ColumnKind::Integer,
                    14.0,
                    110.0,
                ),
                ReportColumn::new("amount", "支付金额", ColumnKind::Money, 20.0, 170.0),
                ReportColumn::new("share", "占合计", ColumnKind::Rate, 18.0, 150.0),
            ]
        } else {
            vec![
                ReportColumn::new(
                    "group_label",
                    dim.key_title(),
                    ColumnKind::Text,
                    30.0,
                    200.0,
                ),
                ReportColumn::new(
                    "payment_count",
                    "单据笔数",
                    ColumnKind::Integer,
                    14.0,
                    110.0,
                ),
                ReportColumn::new("amount", "支付金额", ColumnKind::Money, 20.0, 170.0),
                ReportColumn::new("share", "占合计", ColumnKind::Rate, 18.0, 150.0),
            ]
        };
        columns[0].screen_remainder = true;

        let total: i64 = Money::sum_saturating(rows.iter().map(|r| r.amount.cent()));
        let data: Vec<Vec<Cell>> = rows
            .iter()
            .map(|r| {
                // 按合同类型的行名是代码（CONSTRUCT…），翻成中文给人看
                let label = if dim == SummaryDim::ContractType {
                    ContractType::from_code(&r.label)
                        .map(|t| t.label().to_string())
                        .unwrap_or_else(|| r.label.clone())
                } else {
                    r.label.clone()
                };
                let bp = ratio_bp(r.amount.cent(), total);
                let mut cells = vec![Cell::Text(label)];
                if dim == SummaryDim::Contract {
                    cells.push(Cell::Text(r.sub.clone().unwrap_or_default()));
                }
                cells.push(Cell::Integer(r.payment_count));
                cells.push(Cell::Money(r.amount));
                cells.push(Cell::Rate(percent_text(bp), bp));
                cells
            })
            .collect();
        let total_bp = if total > 0 { 10_000 } else { 0 };
        let mut totals = vec![Cell::Text("合计".to_string())];
        if dim == SummaryDim::Contract {
            totals.push(Cell::Text(String::new()));
        }
        totals.push(Cell::Integer(Money::sum_saturating(
            rows.iter().map(|r| r.payment_count),
        )));
        totals.push(Cell::Money(Money::from_cent(total)));
        totals.push(Cell::Rate(percent_text(total_bp), total_bp));

        let filter_part = if filter_note.is_empty() {
            String::new()
        } else {
            format!("；筛选：{filter_note}")
        };
        // 区间留空 = 不限；副标题里写明，避免出现「统计区间  ~ 」的空壳
        let from_txt = if from.trim().is_empty() {
            "不限"
        } else {
            from
        };
        let to_txt = if to.trim().is_empty() { "不限" } else { to };
        ReportResult {
            kind,
            title: kind.title(),
            subtitle: format!(
                "{}（统计区间 {from_txt} ~ {to_txt}）{filter_part}",
                kind.subtitle()
            ),
            unit_note: "金额单位：元",
            columns,
            rows: data,
            totals,
            generated_at: now_iso(),
        }
    }

    /// 合同台账（R-33）：列配置与屏幕列一一对应，`visible` 控制窄屏降列。
    /// 只有数据列进结果，点击 / 进度条 / 徽标等交互仍由页面按 `columns[i].key` 绘制。
    pub fn contract_ledger(
        rows: &[ctp_core::ledger::ContractSummary],
        visible: LedgerColumns,
    ) -> Self {
        let kind = ReportKind::ContractLedger;
        let mut columns = vec![
            ReportColumn::new("contract_no", "合同编号", ColumnKind::Text, 14.0, 90.0),
            ReportColumn::new("contract_name", "合同名称", ColumnKind::Text, 34.0, 120.0),
        ];
        if visible.wide {
            columns.push(ReportColumn::new(
                "contractor_name",
                "承包单位",
                ColumnKind::Text,
                22.0,
                165.0,
            ));
            columns.push(ReportColumn::new(
                "total_amount",
                "合同金额",
                ColumnKind::Money,
                18.0,
                92.0,
            ));
        }
        if visible.medium {
            columns.push(ReportColumn::new(
                "audit_price",
                "审计价",
                ColumnKind::Money,
                18.0,
                92.0,
            ));
            columns.push(ReportColumn::new(
                "delivery_date",
                "交工日期",
                ColumnKind::Text,
                14.0,
                88.0,
            ));
        }
        columns.push(ReportColumn::new(
            "paid_amount",
            "累计已付",
            ColumnKind::Money,
            18.0,
            104.0,
        ));
        columns.push(ReportColumn::new(
            "pay_rate",
            "支付率",
            ColumnKind::Rate,
            14.0,
            128.0,
        ));
        columns.push(ReportColumn::new(
            "project_status",
            "项目状态",
            ColumnKind::Text,
            12.0,
            72.0,
        ));
        columns[1].screen_remainder = true;

        let data: Vec<Vec<Cell>> = rows
            .iter()
            .map(|s| {
                let mut cells = vec![
                    Cell::Text(s.contract_no.clone()),
                    Cell::Text(s.contract_name.clone()),
                ];
                if visible.wide {
                    cells.push(Cell::Text(s.contractor_name.clone()));
                    cells.push(Cell::Money(s.total_amount));
                }
                if visible.medium {
                    cells.push(match s.audit_price {
                        Some(m) => Cell::Money(m),
                        None => Cell::Text("—".to_string()),
                    });
                    cells.push(Cell::Text(
                        s.delivery_date.clone().unwrap_or_else(|| "—".into()),
                    ));
                }
                cells.push(Cell::Money(s.paid_amount));
                cells.push(Cell::Rate(s.pay_rate.percent_string(), s.pay_rate.bp()));
                cells.push(Cell::Text(
                    if s.is_archived() {
                        "归档关闭"
                    } else {
                        "履行中"
                    }
                    .to_string(),
                ));
                cells
            })
            .collect();

        let sum = |f: fn(&ctp_core::ledger::ContractSummary) -> i64| -> i64 {
            Money::sum_saturating(rows.iter().map(f))
        };
        let total_amount = sum(|s| s.total_amount.cent());
        let audit = sum(|s| s.audit_price.map(|m| m.cent()).unwrap_or(0));
        let paid = sum(|s| s.paid_amount.cent());
        let base = sum(|s| s.base_amount.cent());
        let total_bp = ratio_bp(paid, base);
        let mut totals = vec![Cell::Text("合计".to_string()), Cell::Text(String::new())];
        if visible.wide {
            totals.push(Cell::Text(String::new()));
            totals.push(Cell::Money(Money::from_cent(total_amount)));
        }
        if visible.medium {
            totals.push(Cell::Money(Money::from_cent(audit)));
            totals.push(Cell::Text(String::new()));
        }
        totals.push(Cell::Money(Money::from_cent(paid)));
        totals.push(Cell::Rate(percent_text(total_bp), total_bp));
        totals.push(Cell::Text(String::new()));

        ReportResult {
            kind,
            title: kind.title(),
            subtitle: format!("{}（共 {} 份合同）", kind.subtitle(), rows.len()),
            unit_note: "金额单位：元",
            columns,
            rows: data,
            totals,
            generated_at: now_iso(),
        }
    }

    /// 拨款一览（R-33）：按合同筛出的单据；勾选列与操作列属于交互，不进结果。
    /// 合计只累加已实付单据，作废单保留行但不计入金额。
    pub fn payment_ledger(rows: &[ctp_core::payment::Payment], contract_label: &str) -> Self {
        Self::payment_ledger_impl(rows, contract_label, None)
    }

    /// 拨款一览 **Excel 导出专用**（R-46）：与明细表共用同一套取数 / 合计逻辑，
    /// 额外追加「合同名称」「承包单位」两列，按每行 `contract_id` 查表填充。
    ///
    /// 查不到对应合同、或字段本身为空时写空串，不跳过行、不报错；
    /// 屏幕明细表仍走 `payment_ledger`，列数保持 5 列不变。
    pub fn payment_ledger_export(
        rows: &[ctp_core::payment::Payment],
        contract_label: &str,
        lookup: &PaymentContractLookup,
    ) -> Self {
        Self::payment_ledger_impl(rows, contract_label, Some(lookup))
    }

    fn payment_ledger_impl(
        rows: &[ctp_core::payment::Payment],
        contract_label: &str,
        export_lookup: Option<&PaymentContractLookup>,
    ) -> Self {
        use ctp_core::payment::PaymentStatus;
        let kind = ReportKind::PaymentLedger;
        let mut columns = vec![
            ReportColumn::new("payment_no", "拨款单号", ColumnKind::Text, 16.0, 104.0),
            ReportColumn::new("pay_date", "开单日期", ColumnKind::Text, 13.0, 80.0),
            ReportColumn::new("amount", "金额", ColumnKind::Money, 18.0, 92.0),
            ReportColumn::new("reason", "事由", ColumnKind::Text, 26.0, 60.0),
            ReportColumn::new("status", "状态", ColumnKind::Text, 10.0, 54.0),
        ];
        columns[3].screen_remainder = true;
        if export_lookup.is_some() {
            columns.push(ReportColumn::new(
                "contract_name",
                "合同名称",
                ColumnKind::Text,
                34.0,
                150.0,
            ));
            columns.push(ReportColumn::new(
                "contractor_name",
                "承包单位",
                ColumnKind::Text,
                22.0,
                120.0,
            ));
        }

        let total: i64 = Money::sum_saturating(
            rows.iter()
                .filter(|p| p.status != PaymentStatus::Void)
                .map(|p| p.amount_cent.cent()),
        );
        let data: Vec<Vec<Cell>> = rows
            .iter()
            .map(|p| {
                let voided = p.status == PaymentStatus::Void;
                let mut cells = vec![
                    Cell::Text(p.payment_no.clone()),
                    Cell::Text(p.pay_date.clone()),
                    Cell::Money(p.amount_cent),
                    Cell::Text(p.reason.clone().unwrap_or_else(|| "—".into())),
                    Cell::Text(if voided { "作废" } else { "已付" }.to_string()),
                ];
                if let Some(lookup) = export_lookup {
                    let found = lookup.get(&p.contract_id);
                    cells.push(Cell::Text(
                        found
                            .map(|f| f.contract_name.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    ));
                    cells.push(Cell::Text(
                        found
                            .map(|f| f.contractor_name.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    ));
                }
                cells
            })
            .collect();
        let mut totals = vec![
            Cell::Text("合计".to_string()),
            Cell::Text(String::new()),
            Cell::Money(Money::from_cent(total)),
            Cell::Text(String::new()),
            Cell::Text(String::new()),
        ];
        if export_lookup.is_some() {
            totals.push(Cell::Text(String::new()));
            totals.push(Cell::Text(String::new()));
        }

        ReportResult {
            kind,
            title: kind.title(),
            subtitle: format!(
                "{}；合同：{}；共 {} 笔（作废单不计入合计）",
                kind.subtitle(),
                contract_label,
                rows.len()
            ),
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

/// 整数万分比，避免 f64 参与业务比率运算。
///
/// R-39：唯一实现委托 `Rate::ratio`，与 SQL 视图 `pay_rate_bp`、`selfcheck`
/// 及主业务比率保持同一套四舍五入口径；旧版这里用整数除法直接截断，
/// 会让报表占比比主业务少 1 bp。分母 <= 0 由 `Rate::ratio` 统一返回 0。
pub fn ratio_bp(num: i64, den: i64) -> i32 {
    ctp_core::rate::Rate::ratio(Money::from_cent(num), Money::from_cent(den)).bp()
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

    /// R-39：报表比率与 `Rate::ratio`（含 SQL 视图）同口径，
    /// 半值要进位、负分母要返回 0，不再用整数除法截断。
    #[test]
    fn 比率统一到四舍五入且边界安全() {
        // 0.5 bp：旧实现截断成 0，统一后按四舍五入得 1
        assert_eq!(ratio_bp(1, 20_000), 1);
        assert_eq!(ratio_bp(3, 20_000), 2, "1.5 bp 应进位到 2");
        // 半值不进位的一侧
        assert_eq!(ratio_bp(1, 30_000), 0, "0.333 bp 应为 0");
        // 分母 <= 0：统一返回 0（旧实现对负分母会给出负数）
        assert_eq!(ratio_bp(5, 0), 0);
        assert_eq!(ratio_bp(5, -10), 0);
        // 逐位一致：所有取样点都等于 Rate::ratio
        for (n, d) in [
            (0, 1),
            (1, 3),
            (100, 300),
            (150, 300),
            (300, 300),
            (-100, 300),
        ] {
            assert_eq!(
                ratio_bp(n, d),
                Rate::ratio(Money::from_cent(n), Money::from_cent(d)).bp(),
                "ratio_bp({n},{d}) 必须与 Rate::ratio 一致"
            );
        }
    }

    // ------------------------------------------------------------ R-46 导出列选择

    #[test]
    fn 列选择按指定顺序裁剪且保留取值与合计() {
        let r = ReportResult::period(&period_rows());
        let picked = r.select_columns(&["amount", "period_month"]).unwrap();
        assert_eq!(
            picked.columns.iter().map(|c| c.title).collect::<Vec<_>>(),
            vec!["支付金额", "期间"]
        );
        assert_eq!(picked.columns[0].excel_title(), "支付金额（元）");
        assert_eq!(picked.columns[1].excel_title(), "期间");
        assert_eq!(picked.rows.len(), r.rows.len());
        assert_eq!(picked.rows[0][0].yuan(), Some(300_000.0));
        assert_eq!(picked.rows[0][1].display(), "2026-06");
        assert_eq!(picked.rows[1][1].display(), "2026-07");
        assert_eq!(picked.totals[0].yuan(), Some(400_000.0));
        assert_eq!(picked.totals[1].display(), "合计");
        assert_eq!(picked.kind, r.kind);
        assert_eq!(picked.subtitle, r.subtitle);
        picked.validate_shape().unwrap();
    }

    #[test]
    fn 列选择校验挡住空重复未知与行列错位() {
        let r = ReportResult::period(&period_rows());
        assert_eq!(
            r.select_columns(&[]),
            Err(ReportColumnError::EmptySelection)
        );
        assert_eq!(
            r.select_columns(&["amount", "amount"]),
            Err(ReportColumnError::Duplicate("amount".into()))
        );
        assert_eq!(
            r.select_columns(&["不存在的列"]),
            Err(ReportColumnError::Unknown("不存在的列".into()))
        );
        let mut short = r.clone();
        short.rows[0].pop();
        assert_eq!(
            short.select_columns(&["amount"]),
            Err(ReportColumnError::RowShape {
                row: 0,
                actual: 3,
                expected: 4
            })
        );
        let mut bad_totals = r.clone();
        bad_totals.totals.pop();
        assert_eq!(
            bad_totals.select_columns(&["amount"]),
            Err(ReportColumnError::TotalsShape {
                actual: 3,
                expected: 4
            })
        );
    }

    fn payment_rows() -> Vec<ctp_core::payment::Payment> {
        use ctp_core::payment::Payment;
        vec![
            Payment {
                id: "PM-1".into(),
                payment_no: "BK-0001".into(),
                contract_id: "CT-1".into(),
                pay_date: "2026-06-30".into(),
                amount_cent: Money::from_cent(1_000_000),
                ..Default::default()
            },
            Payment {
                id: "PM-2".into(),
                payment_no: "BK-0002".into(),
                // 查找表里没有这份合同：两列都应留空，且不影响本行其他字段
                contract_id: "CT-UNKNOWN".into(),
                pay_date: "2026-07-01".into(),
                amount_cent: Money::from_cent(500_000),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn 拨款一览导出补齐合同字段且缺失留空() {
        let rows = payment_rows();
        let mut lookup = PaymentContractLookup::new();
        lookup.insert(
            "CT-1".into(),
            PaymentContractFields {
                contract_name: "测试合同".into(),
                contractor_name: "某施工单位".into(),
            },
        );
        let screen = ReportResult::payment_ledger(&rows, "全部合同");
        assert_eq!(screen.columns.len(), 5, "屏幕明细表仍保持 5 列");
        let exported = ReportResult::payment_ledger_export(&rows, "全部合同", &lookup);
        assert_eq!(exported.columns.len(), 7);
        assert_eq!(exported.columns[5].title, "合同名称");
        assert_eq!(exported.columns[6].title, "承包单位");
        assert_eq!(exported.rows[0][5].display(), "测试合同");
        assert_eq!(exported.rows[0][6].display(), "某施工单位");
        assert_eq!(exported.rows[1][5].display(), "", "查不到合同应留空");
        assert_eq!(exported.rows[1][6].display(), "", "查不到合同应留空");
        assert_eq!(exported.rows[1][0].display(), "BK-0002");
        assert_eq!(exported.totals.len(), 7);
        assert_eq!(exported.totals[2].yuan(), Some(15_000.0));
        screen.validate_shape().unwrap();
        exported.validate_shape().unwrap();
    }
}
