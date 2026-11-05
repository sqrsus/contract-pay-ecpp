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
        ColumnKind::Integer => f
            .set_num_format("#,##0")
            .set_align(FormatAlign::Right),
        ColumnKind::Money => f
            .set_num_format("#,##0.00")
            .set_align(FormatAlign::Right),
        ColumnKind::Rate => f
            .set_num_format("0.00\"%\"")
            .set_align(FormatAlign::Right),
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
pub fn write_report(
    result: &ReportResult,
    path: &Path,
    operator: &str,
) -> Result<(usize, usize)> {
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

    // 第 3 行：表头（金额列注明单位「元」）
    for (ci, c) in result.columns.iter().enumerate() {
        let label = if c.kind == ColumnKind::Money {
            format!("{}（元）", c.title)
        } else {
            c.title.to_string()
        };
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
        "&L导出人：{operator} &C导出时间：{} &R系统版本：0.1.0",
        result.generated_at
    );
    ws.set_footer(footer.as_str());

    workbook.save(path).map_err(xerr)?;
    Ok((result.rows.len(), ncols))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::ReportResult;
    use ctp_core::ledger::PeriodRow;
    use ctp_core::money::Money;

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
}
