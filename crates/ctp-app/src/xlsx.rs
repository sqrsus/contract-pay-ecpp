//! `rust_xlsxwriter` 写出：把 `ReportResult` 变成一份 .xlsx。
//!
//! 这一层是**新建式导出**（write-only），不读取也不修改任何已有 .xlsx 模板；
//! 打印模板填充走 `print::PrintAdapter`，是另一条链路，二者不共用实现。
//!
//! 版式：标题行 + 口径说明行 + 表头行（冻结） + 数据行 + 合计行（SUM 公式）。
//! 金额一律以「元」写数值并套 `#,##0.00`，不写字符串，便于用户在 Excel 里二次计算。

use crate::report::{Cell, ColumnKind, ReportResult};
use ctp_store::{Result, StoreError};
use rust_xlsxwriter::{Color, Format, FormatAlign, Formula, Workbook};
use std::path::Path;

fn xerr(e: impl std::fmt::Display) -> StoreError {
    StoreError::Export(e.to_string())
}

/// 单元格格式由列语义驱动（文档 4.4.2「列配置驱动」）。
fn cell_format(kind: ColumnKind, bold: bool) -> Format {
    let mut f = Format::new();
    if bold {
        f = f.set_bold().set_background_color(Color::RGB(0xEFF4FB));
    }
    match kind {
        ColumnKind::Text => f,
        ColumnKind::Integer => f.set_num_format("#,##0").set_align(FormatAlign::Right),
        ColumnKind::Money => f.set_num_format("#,##0.00").set_align(FormatAlign::Right),
        ColumnKind::Rate => f.set_num_format("0.00\"%\"").set_align(FormatAlign::Right),
    }
}

/// 0-based 列号 → Excel 列字母（A, B, …, Z, AA）。
fn col_letter(col: u16) -> String {
    let mut n = col + 1;
    let mut s = String::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.insert(0, (b'A' + r as u8) as char);
        n = (n - 1) / 26;
    }
    s
}

/// 写出报表。返回已写入的行列数，供集成测试断言。
pub fn write_report(result: &ReportResult, path: &Path, operator: &str) -> Result<(usize, usize)> {
    // 先校验形状：列选择 / 追加列之后若行列错位，这里返回可展示的导出错误，
    // 而不是在下面的 columns[ci] 处越界 panic。
    result.validate_shape().map_err(xerr)?;
    let mut workbook = Workbook::new();
    let ws = workbook.add_worksheet();
    ws.set_name(result.kind.sheet_name()).map_err(xerr)?;

    let ncols = result.columns.len();
    let last_col = (ncols.saturating_sub(1)) as u16;
    let title_fmt = Format::new().set_bold();
    let note_fmt = Format::new().set_font_color(Color::RGB(0x6B7280));
    let header_fmt = Format::new()
        .set_bold()
        .set_font_color(Color::RGB(0xFFFFFF))
        .set_background_color(Color::RGB(0x2F5597))
        .set_align(FormatAlign::Center);

    // 第 1 行：标题 + 单位说明；第 2 行：口径 + 导出时间
    ws.merge_range(
        0,
        0,
        0,
        last_col,
        &format!("{}（{}）", result.title, result.unit_note),
        &title_fmt,
    )
    .map_err(xerr)?;
    ws.merge_range(
        1,
        0,
        1,
        last_col,
        &format!(
            "{} ｜ 库内以「分」存储，导出时换算为元 ｜ 导出时间 {} ｜ 数据源 {}",
            result.subtitle,
            result.generated_at,
            result.kind.code()
        ),
        &note_fmt,
    )
    .map_err(xerr)?;

    // 第 3 行：表头（金额列注明单位「元」，文字由列配置统一给出）
    for (ci, c) in result.columns.iter().enumerate() {
        let label = c.excel_title();
        ws.write_string_with_format(2, ci as u16, label.as_str(), &header_fmt)
            .map_err(xerr)?;
        ws.set_column_width(ci as u16, c.width).map_err(xerr)?;
    }

    const HEADER_ROWS: u32 = 3;
    let first_data_row = HEADER_ROWS;
    let last_data_row = first_data_row + result.rows.len() as u32; // 开区间

    // 数据行
    for (ri, row) in result.rows.iter().enumerate() {
        let r = first_data_row + ri as u32;
        for (ci, cell) in row.iter().enumerate() {
            let c = ci as u16;
            let kind = result.columns[ci].kind;
            let fmt = cell_format(kind, false);
            match cell {
                Cell::Text(_) => ws
                    .write_string_with_format(r, c, cell.display(), &fmt)
                    .map_err(xerr)?,
                Cell::Integer(v) => ws
                    .write_number_with_format(r, c, *v as f64, &fmt)
                    .map_err(xerr)?,
                Cell::Money(_) => ws
                    .write_number_with_format(r, c, cell.yuan().unwrap_or(0.0), &fmt)
                    .map_err(xerr)?,
                Cell::Rate(_, bp) => ws
                    .write_number_with_format(r, c, *bp as f64 / 100.0, &fmt)
                    .map_err(xerr)?,
            };
        }
    }

    // 合计行：数字列写 SUM 公式（带缓存结果），文本列写标签
    let total_row = last_data_row;
    for (ci, cell) in result.totals.iter().enumerate() {
        let c = ci as u16;
        let kind = result.columns[ci].kind;
        let fmt = cell_format(kind, true);
        match kind {
            ColumnKind::Integer | ColumnKind::Money => {
                let letter = col_letter(c);
                let formula = format!(
                    "=SUM({}{}:{}{})",
                    letter,
                    first_data_row + 1,
                    letter,
                    last_data_row
                );
                let cached = match cell {
                    Cell::Money(_) => cell.yuan().unwrap_or(0.0).to_string(),
                    _ => cell.integer().unwrap_or(0).to_string(),
                };
                let f = Formula::new(formula).set_result(cached);
                ws.write_formula_with_format(total_row, c, f, &fmt)
                    .map_err(xerr)?;
            }
            ColumnKind::Rate => {
                let bp = cell.rate_bp().unwrap_or(0);
                ws.write_number_with_format(total_row, c, bp as f64 / 100.0, &fmt)
                    .map_err(xerr)?;
            }
            ColumnKind::Text => {
                ws.write_string_with_format(total_row, c, cell.display(), &fmt)
                    .map_err(xerr)?;
            }
        }
    }

    // 冻结标题 / 口径 / 表头三行，横向滚动时表头不动
    ws.set_freeze_panes(HEADER_ROWS, 0).map_err(xerr)?;
    ws.set_landscape();
    let footer = format!(
        "&L导出人：{operator} &C导出时间：{} &R系统版本：{}",
        result.generated_at,
        env!("CARGO_PKG_VERSION"),
    );
    ws.set_footer(footer.as_str());

    // R-68（#13）：先写临时文件再原子改名，覆盖已有导出时不留半截。
    // 直接 `save(path)` 写一半失败（磁盘满 / 被占用）会把目标文件毁成打不开的
    // 残档；`std::fs::rename` 在 Windows 上带 REPLACE_EXISTING，替换是原子的。
    let tmp = std::path::PathBuf::from(format!("{}.tmp", path.display()));
    if let Err(e) = workbook.save(&tmp) {
        // 临时文件写坏了就顺手清掉，别在导出目录留一堆 .tmp 残档
        let _ = std::fs::remove_file(&tmp);
        return Err(xerr(e));
    }
    std::fs::rename(&tmp, path).map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        xerr(format!(
            "替换导出文件失败（可能正被 Excel 打开）：{}",
            path.display()
        ))
    })?;
    Ok((result.rows.len(), ncols))
}

#[cfg(test)]
mod tests {
    // 12_345_67 = 12,345.67 元，按「元_分」分组便于人工核对导出值。
    #![allow(clippy::inconsistent_digit_grouping)]
    use super::*;
    use crate::report::{PaymentContractFields, PaymentContractLookup, ReportResult};
    use ctp_core::ledger::PeriodRow;
    use ctp_core::money::Money;

    fn tmp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ctp_r46_{}_{}.xlsx", std::process::id(), tag))
    }

    /// 读回某个单元格的文字（calamine 在 ctp-app 里本来就是导入依赖）。
    fn cell_text(v: Option<&calamine::Data>) -> String {
        match v {
            Some(calamine::Data::String(s)) => s.clone(),
            Some(calamine::Data::Float(f)) => f.to_string(),
            Some(calamine::Data::Int(i)) => i.to_string(),
            Some(calamine::Data::Empty) | None => String::new(),
            Some(other) => format!("{other:?}"),
        }
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
                contract_id: "CT-UNKNOWN".into(),
                pay_date: "2026-07-01".into(),
                amount_cent: Money::from_cent(500_000),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn 列字母与格式工厂() {
        assert_eq!(col_letter(0), "A");
        assert_eq!(col_letter(25), "Z");
        assert_eq!(col_letter(26), "AA");
        let _ = cell_format(ColumnKind::Money, true);
    }

    #[test]
    fn 空报表也能写出可打开的xlsx() {
        let rows = vec![PeriodRow {
            project_id: "P1".into(),
            period_month: "2026-06".into(),
            payment_count: 1,
            amount: Money::from_cent(12_345_67),
        }];
        let result = ReportResult::period(&rows);
        let path = std::env::temp_dir().join(format!(
            "ctp_report_{}_{}.xlsx",
            std::process::id(),
            result.generated_at.replace([':', '-', 'T', 'Z'], "")
        ));
        let (nrows, ncols) = write_report(&result, &path, "测试").unwrap();
        assert_eq!(nrows, 1);
        assert_eq!(ncols, 4);
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.len() > 1_000, "xlsx 体积异常：{}", meta.len());
        // xlsx 是 zip：头两个字节必须是 PK
        let head = std::fs::read(&path).unwrap();
        assert_eq!(&head[0..2], b"PK");
        let _ = std::fs::remove_file(&path);
    }

    /// R-68（#13）：覆盖已有导出走「临时文件 + 原子改名」，
    /// 目标始终是完整 xlsx，且导出目录不留 .tmp 残档。
    #[test]
    fn 覆盖导出走原子改名且不留临时文件() {
        use calamine::Reader;
        let path = tmp_path("atomic");
        let _ = std::fs::remove_file(&path);
        let result = ReportResult::period(&period_rows());
        write_report(&result, &path, "测试").unwrap();
        let first = std::fs::read(&path).unwrap();
        assert_eq!(&first[0..2], b"PK");

        let mut short = period_rows();
        short.truncate(1);
        let small = ReportResult::period(&short);
        assert_eq!(write_report(&small, &path, "测试").unwrap(), (1, 4));

        let after = std::fs::read(&path).unwrap();
        assert_eq!(&after[0..2], b"PK", "覆盖后仍必须是完整 xlsx");
        assert_ne!(after, first, "覆盖后内容应换成新报表");
        assert!(
            !std::path::PathBuf::from(format!("{}.tmp", path.display())).exists(),
            "不能留 .tmp 残档"
        );
        let mut wb: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
        let range = wb.worksheet_range("期间统计").unwrap();
        assert!(
            !cell_text(range.get_value((2, 0))).is_empty(),
            "覆盖后的文件必须仍能正常打开并读到表头"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// R-46：列选择后写出的 xlsx 只含所选列，表头文字与顺序与选择一致。
    #[test]
    fn 列选择导出只写所选列且表头顺序一致() {
        use calamine::Reader;
        let result = ReportResult::period(&period_rows());
        let picked = result.select_columns(&["amount", "period_month"]).unwrap();
        let path = tmp_path("select");
        let (nrows, ncols) = write_report(&picked, &path, "测试").unwrap();
        assert_eq!((nrows, ncols), (2, 2));

        let mut wb: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
        let range = wb.worksheet_range("期间统计").unwrap();
        assert_eq!(cell_text(range.get_value((2, 0))), "支付金额（元）");
        assert_eq!(cell_text(range.get_value((2, 1))), "期间");
        assert_eq!(cell_text(range.get_value((2, 2))), "", "不应出现第 3 列");
        assert_eq!(cell_text(range.get_value((3, 0))), "300000");
        assert_eq!(cell_text(range.get_value((3, 1))), "2026-06");
        assert_eq!(cell_text(range.get_value((4, 1))), "2026-07");
        let _ = std::fs::remove_file(&path);
    }

    /// R-46：拨款一览导出的两列按记录填充，缺失值留空且不丢行。
    #[test]
    fn 拨款一览导出带合同字段且缺失为空() {
        use calamine::Reader;
        let mut lookup = PaymentContractLookup::new();
        lookup.insert(
            "CT-1".into(),
            PaymentContractFields {
                contract_name: "测试合同".into(),
                contractor_name: "某施工单位".into(),
            },
        );
        let result = ReportResult::payment_ledger_export(&payment_rows(), "全部合同", &lookup);
        let path = tmp_path("payment");
        let (nrows, ncols) = write_report(&result, &path, "测试").unwrap();
        assert_eq!((nrows, ncols), (2, 7));

        let mut wb: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
        let range = wb.worksheet_range("拨款一览").unwrap();
        assert_eq!(cell_text(range.get_value((2, 5))), "合同名称");
        assert_eq!(cell_text(range.get_value((2, 6))), "承包单位");
        assert_eq!(cell_text(range.get_value((3, 5))), "测试合同");
        assert_eq!(cell_text(range.get_value((3, 6))), "某施工单位");
        assert_eq!(cell_text(range.get_value((4, 5))), "");
        assert_eq!(cell_text(range.get_value((4, 6))), "");
        assert_eq!(cell_text(range.get_value((4, 0))), "BK-0002");
        let _ = std::fs::remove_file(&path);
    }

    /// 行列错位的结果在写出前被挡住，而不是在 columns[ci] 处越界。
    #[test]
    fn 形状错位写出前返回错误而不是panic() {
        let mut result = ReportResult::period(&[]);
        result.rows.push(vec![Cell::Text("只有一个单元格".into())]);
        let path = tmp_path("bad_shape");
        let err = write_report(&result, &path, "测试").unwrap_err();
        assert!(err.to_string().contains("第 1 行"), "{err}");
        let _ = std::fs::remove_file(&path);
    }
}
