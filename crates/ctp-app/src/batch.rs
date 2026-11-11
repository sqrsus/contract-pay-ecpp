//! 批量拨款：导出填表模板（xlsx）与导入回传。
//!
//! **校验标记**：模板里带一张隐藏工作表 `_CTP_CHECK`，A1 写死 `CHECK_MARK`。
//! 导入时先比对这张表与标记，读不到或不一致就直接拒绝——避免用户拿别的表
//! 改改列名就传上来，导致列错位写坏账。
//!
//! **前 5 列只作核对**：合同名称以外的名称/价/已付都是导出当时的快照，
//! 导入时一律不采信（以库内为准），只有后 4 列（本次金额 / 开单日期 / 事由 /
//! 支付方式）参与写入。这样即使用户在预填格里改数字也不会影响账。
//!
//! **逐行提交**：每一行单独走 `service::register_payment`（内部独立事务），
//! 失败的行跳过并记下行号，成功的行已经落库——按 2026-11-06 确认的口径。
//!
//! **导入幂等**：每行按「Excel 绝对行号 + 该行所有单元格原值」算 SHA-256 作
//! `import_key` 写进拨款单。同一份表重复上传时已入账的行直接跳过，不会重复拨款；
//! 用户改了某一行内容才算新数据。日期留空按当天展开，但指纹取的是原始空值，
//! 隔天重传同一张表仍能被识别为同一行。

use crate::service::{self, NewPayment};
use ctp_core::date::{is_iso_date, today_local_iso};
use ctp_core::digest;
use ctp_core::ids;
use ctp_core::money::Money;
use ctp_core::payment::PayMethod;
use ctp_store::{db, repo, Result, StoreError};
use rusqlite::Connection;
use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, Workbook};
use std::collections::HashMap;
use std::path::Path;

/// 模板主工作表名。
pub const TEMPLATE_SHEET: &str = "批量拨款";

/// 校验标记所在的工作表（隐藏）。
pub const CHECK_SHEET: &str = "_CTP_CHECK";

/// 校验标记内容；改它等于让所有旧模板失效，不要动。
pub const CHECK_MARK: &str = "CTP-BATCH-PAYMENT-V1";

/// 模板列标题（9 列）。前 5 列预填，后 4 列由用户填写。
pub const HEADERS: [&str; 9] = [
    "合同名称",
    "承包单位",
    "合同价（元）",
    "审计价（元）",
    "累计已付（元）",
    "本次金额（元）",
    "开单日期",
    "事由",
    "支付方式",
];

/// 数据起始行（0-based）：第 1 行标题、第 2 行说明、第 3 行表头。
const FIRST_DATA_ROW: usize = 3;

/// 导入文件大小上限（10 MB）：xlsx 是压缩包，解压后可能放大几十倍，
/// 不设上限会让一次误选大文件把 UI 帧长时间卡死（R-39）。
pub const MAX_IMPORT_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// 导入行数上限（5000 行）：逐行提交是同步的，行数必须有上界。
pub const MAX_IMPORT_ROWS: usize = 5_000;

/// 列宽（字符）。
const COL_WIDTHS: [f64; 9] = [34.0, 22.0, 15.0, 15.0, 15.0, 15.0, 13.0, 26.0, 11.0];

fn xerr(e: impl std::fmt::Display) -> StoreError {
    StoreError::Export(e.to_string())
}

/// 模板里的一行（一份合同）。
pub struct TemplateRow {
    pub contract_name: String,
    pub contractor: String,
    pub contract_amount: String,
    pub audit_amount: String,
    pub paid_amount: String,
}

impl TemplateRow {
    /// 从 V1 汇总行取数：合同价、审计价、累计已付都按系统口径，不让用户自己填。
    pub fn from_summary(s: &ctp_core::ledger::ContractSummary) -> Self {
        TemplateRow {
            contract_name: s.contract_name.clone(),
            contractor: s.contractor_name.clone(),
            contract_amount: s.total_amount.fixed_yuan(),
            audit_amount: s
                .audit_price
                .map(|m| m.fixed_yuan())
                .unwrap_or_else(|| "0.00".into()),
            paid_amount: s.paid_amount.fixed_yuan(),
        }
    }
}

/// 写出模板。返回（合同行数, 列数）。
pub fn write_template(
    rows: &[TemplateRow],
    path: &Path,
    generated_at: &str,
) -> Result<(usize, usize)> {
    let mut workbook = Workbook::new();

    {
        let ws = workbook.add_worksheet();
        ws.set_name(TEMPLATE_SHEET).map_err(xerr)?;

        let title_fmt = Format::new().set_bold().set_font_size(14.0);
        let note_fmt = Format::new()
            .set_font_color(Color::RGB(0x6B7280))
            .set_font_size(10.0);
        let header_fmt = Format::new()
            .set_bold()
            .set_font_color(Color::RGB(0xFFFFFF))
            .set_background_color(Color::RGB(0x2F5597))
            .set_align(FormatAlign::Center)
            .set_text_wrap();
        // 前 5 列是系统快照，灰底提示「别改也无效」
        let prefill_fmt = Format::new()
            .set_font_color(Color::RGB(0x6B7280))
            .set_background_color(Color::RGB(0xF3F4F6));
        let prefill_money_fmt = Format::new()
            .set_font_color(Color::RGB(0x6B7280))
            .set_background_color(Color::RGB(0xF3F4F6))
            .set_num_format("#,##0.00")
            .set_align(FormatAlign::Right);
        // 后 4 列需要填写，留白 + 描边
        let input_fmt = Format::new().set_border(FormatBorder::Thin);
        let input_date_fmt = Format::new()
            .set_border(FormatBorder::Thin)
            .set_num_format("@");
        let input_money_fmt = Format::new()
            .set_border(FormatBorder::Thin)
            .set_num_format("#,##0.00")
            .set_align(FormatAlign::Right);

        let last_col = (HEADERS.len() - 1) as u16;
        ws.merge_range(0, 0, 0, last_col, "批量拨款导入模板", &title_fmt)
            .map_err(xerr)?;
        ws.merge_range(
            1,
            0,
            1,
            last_col,
            &format!(
                "只填后 4 列；前 5 列是导出时点的系统快照，导入时不采信。\
                 本次金额为 0 或留空的行跳过；开单日期留空按当天（YYYY-MM-DD）；\
                 事由留空按「拨付进度款」；支付方式留空按转账，也可填 转账/现金/其他。\
                 导出时间 {generated_at}"
            ),
            &note_fmt,
        )
        .map_err(xerr)?;

        for (ci, h) in HEADERS.iter().enumerate() {
            ws.write_string_with_format(2, ci as u16, *h, &header_fmt)
                .map_err(xerr)?;
            ws.set_column_width(ci as u16, COL_WIDTHS[ci])
                .map_err(xerr)?;
        }
        ws.set_row_height(1, 32.0).map_err(xerr)?;

        for (ri, r) in rows.iter().enumerate() {
            let row = FIRST_DATA_ROW as u32 + ri as u32;
            ws.write_string_with_format(row, 0, r.contract_name.as_str(), &prefill_fmt)
                .map_err(xerr)?;
            ws.write_string_with_format(row, 1, r.contractor.as_str(), &prefill_fmt)
                .map_err(xerr)?;
            // 3 个金额写成数值而不是文本：文本数字在 Excel 里会挂绿三角，也不便核对
            for (col, text) in [
                (2u16, &r.contract_amount),
                (3, &r.audit_amount),
                (4, &r.paid_amount),
            ] {
                let v = text.parse::<f64>().unwrap_or(0.0);
                ws.write_number_with_format(row, col, v, &prefill_money_fmt)
                    .map_err(xerr)?;
            }
            // 后 4 列只给格式，不写值
            ws.write_blank(row, 5, &input_money_fmt).map_err(xerr)?;
            ws.write_blank(row, 6, &input_date_fmt).map_err(xerr)?;
            ws.write_blank(row, 7, &input_fmt).map_err(xerr)?;
            ws.write_blank(row, 8, &input_fmt).map_err(xerr)?;
        }

        ws.set_freeze_panes(3, 0).map_err(xerr)?;
    }

    {
        // 隐藏的校验表：只放一个标记串
        let ws = workbook.add_worksheet();
        ws.set_name(CHECK_SHEET).map_err(xerr)?;
        ws.write_string(0, 0, CHECK_MARK).map_err(xerr)?;
        ws.write_string(
            0,
            1,
            "本表由系统生成，请勿删除或修改；导入时用于校验模板来源。",
        )
        .map_err(xerr)?;
        ws.set_hidden(true);
    }

    workbook.save(path).map_err(xerr)?;
    Ok((rows.len(), HEADERS.len()))
}

/// 导入时的一行失败记录（行号是 Excel 里的可见行号，从 1 起）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFailure {
    pub line: usize,
    pub message: String,
}

/// 导入结果：成功多少行、跳过多少行、哪些行失败，以及批次审计信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: usize,
    /// 本次金额为 0 或留空而跳过的行数（不算失败）
    pub skipped: usize,
    pub failures: Vec<ImportFailure>,
    /// 导入幂等键命中、判定为重复而跳过的行数（不算失败）
    pub duplicates: usize,
    /// 本次导入的批次号；批次日志写入失败时据此补记
    pub batch_no: String,
    /// 批次审计日志写入失败等可见警告；非空时界面必须提示
    pub warning: Option<String>,
}

impl ImportReport {
    pub fn has_failure(&self) -> bool {
        !self.failures.is_empty()
    }

    /// 追加一条可见警告（多条自动拼接，不覆盖前一条）。
    pub fn warn(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        match self.warning.as_mut() {
            Some(w) => {
                w.push('；');
                w.push_str(&m);
            }
            None => self.warning = Some(m),
        }
    }

    /// 统计摘要：结果弹窗顶部与提示条共用，不列逐行明细。
    pub fn summary(&self) -> String {
        let dup = if self.duplicates > 0 {
            format!("，重复跳过 {} 行", self.duplicates)
        } else {
            String::new()
        };
        format!(
            "成功导入 {} 笔，跳过 {} 行，失败 {} 行{dup}",
            self.imported,
            self.skipped,
            self.failures.len()
        )
    }
}

/// 行级幂等指纹：Excel 绝对行号 + 该行所有单元格的**原始**文本。
///
/// 只取 row 里的值、不取展开后的业务值（例如留空日期在写入时才补今天），
/// 这样同一张表隔天重传仍算出同一个键；改了任一格才算新数据。
fn row_fingerprint(row: &[calamine::Data], line: usize) -> String {
    let mut buf = format!("line={line}\u{1f}");
    for (i, d) in row.iter().enumerate() {
        buf.push_str(&format!("{i}={}", cell_text(d)));
        buf.push('\u{1f}');
    }
    format!("BKIMP:{}", digest::sha256_hex(buf.as_bytes()))
}

fn cell_text(d: &calamine::Data) -> String {
    match d {
        calamine::Data::Empty => String::new(),
        calamine::Data::String(s) => s.trim().to_string(),
        calamine::Data::Float(f) => {
            if f.fract().abs() < 1e-9 {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        calamine::Data::Int(i) => i.to_string(),
        calamine::Data::Bool(b) => b.to_string(),
        calamine::Data::DateTimeIso(s) => s.trim().to_string(),
        _ => String::new(),
    }
}

fn cell(row: &[calamine::Data], col: usize) -> String {
    row.get(col).map(cell_text).unwrap_or_default()
}

/// 本次金额留空或填 0：该行不导入，也不计入失败行（R-33 确认口径）。
fn cell_amount_skipped(row: &[calamine::Data], col: usize) -> bool {
    let t = cell(row, col);
    t.is_empty() || t.parse::<f64>().map(|v| v.abs() < 1e-9).unwrap_or(false)
}

/// 金额列：既接受数值格，也接受文本（用户可能粘成文本）。
fn cell_money(row: &[calamine::Data], col: usize) -> std::result::Result<Money, String> {
    // 数值格与文本格共用同一个单笔上限，不让「改成数值格式」绕过 parse_yuan 的拦截
    let check = |cent: i64| -> std::result::Result<Money, String> {
        if cent <= 0 {
            return Err("本次金额必须大于 0".into());
        }
        if cent > Money::MAX_SINGLE_CENT {
            return Err(format!(
                "本次金额超过单笔上限 {} 元",
                Money::from_cent(Money::MAX_SINGLE_CENT).format_yuan()
            ));
        }
        Ok(Money::from_cent(cent))
    };
    match row.get(col) {
        Some(calamine::Data::Float(f)) => {
            if !f.is_finite() || *f < 0.0 {
                return Err("本次金额不是有效数字".into());
            }
            let y = (*f * 100.0).round();
            // 浮点转 i64 在 Rust 里会饱和到 i64::MAX，交给 check 统一判超限
            check(y as i64)
        }
        Some(calamine::Data::Int(v)) => {
            let cent = v
                .checked_mul(100)
                .ok_or_else(|| "本次金额过大".to_string())?;
            check(cent)
        }
        Some(calamine::Data::String(s)) => Money::parse_yuan(s).map_err(|e| e.to_string()),
        _ => Err("本次金额为空".into()),
    }
}

fn cell_pay_method(row: &[calamine::Data], col: usize) -> std::result::Result<PayMethod, String> {
    let t = cell(row, col);
    if t.is_empty() {
        return Ok(PayMethod::Transfer);
    }
    for m in [PayMethod::Transfer, PayMethod::Cash, PayMethod::Other] {
        if t == m.label() || t.eq_ignore_ascii_case(m.code()) {
            return Ok(m);
        }
    }
    Err(format!("支付方式「{t}」无法识别，可填 转账/现金/其他"))
}

/// 读取并导入。整张表先过校验标记，再逐行写入。
///
/// 返回 `Err` 表示整份文件不可用（读不出、缺校验表、标记不符）；
/// 行级问题放在 `ImportReport::failures` 里，不影响其他行。
pub fn import(conn: &mut Connection, path: &Path, operator: &str) -> Result<ImportReport> {
    use calamine::Reader;

    // R-39：先把文件大小与行数挡在解压之前/之初，避免超大表把 UI 帧卡死
    let bytes = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| xerr(format!("读取文件信息失败：{e}")))?;
    if bytes > MAX_IMPORT_FILE_BYTES {
        return Err(xerr(format!(
            "文件 {:.1} MB，超过 {} MB 上限；请拆成多个模板分批导入",
            bytes as f64 / (1024.0 * 1024.0),
            MAX_IMPORT_FILE_BYTES / 1024 / 1024
        )));
    }

    let mut wb: calamine::Xlsx<_> = calamine::open_workbook(path).map_err(|e| {
        xerr(format!(
            "打不开这个文件（需要 .xlsx 格式，旧版 .xls 请另存为 .xlsx）：{e}"
        ))
    })?;

    // 1) 校验标记
    if !wb.sheet_names().iter().any(|n| n == CHECK_SHEET) {
        return Err(xerr(
            "缺少校验工作表：这不是本系统导出的模板，请用「导出批量拨款模板」重新导出",
        ));
    }
    let check = wb
        .worksheet_range(CHECK_SHEET)
        .map_err(|e| xerr(format!("读取校验表失败：{e}")))?;
    let mark = check.get_value((0, 0)).map(cell_text).unwrap_or_default();
    if mark != CHECK_MARK {
        return Err(xerr(format!(
            "校验标记不一致（读到「{mark}」，应为「{CHECK_MARK}」），模板可能被改动，已拒绝导入"
        )));
    }

    // 2) 主表
    if !wb.sheet_names().iter().any(|n| n == TEMPLATE_SHEET) {
        return Err(xerr(format!("缺少工作表「{TEMPLATE_SHEET}」")));
    }
    let range = wb
        .worksheet_range(TEMPLATE_SHEET)
        .map_err(|e| xerr(format!("读取「{TEMPLATE_SHEET}」失败：{e}")))?;
    if range.height() > MAX_IMPORT_ROWS {
        return Err(xerr(format!(
            "共 {} 行，超过 {} 行上限；请拆成多个模板分批导入",
            range.height(),
            MAX_IMPORT_ROWS
        )));
    }

    // 3) 合同名称 → 合同 id（同名多份时无法确定，报错而不是猜）
    let summaries = repo::summaries(conn)?;
    let mut by_name: HashMap<&str, Vec<&str>> = HashMap::new();
    for s in &summaries {
        by_name
            .entry(s.contract_name.as_str())
            .or_default()
            .push(s.contract_id.as_str());
    }

    // 批次号在写任何业务数据之前生成：即便日志写失败，界面也能拿到这个号补记。
    // sys_audit_log 是只插不改的事件表，所以用 START / DONE 两条事件描述一次导入，
    // 不尝试「先插后更新」。
    let batch_no = ids::new_id("IMP");
    let file_label = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut report = ImportReport {
        batch_no: batch_no.clone(),
        ..Default::default()
    };
    if let Err(e) = db::insert_audit_log(
        conn,
        "import",
        &batch_no,
        Some(&file_label),
        "IMPORT_START",
        Some(&format!(
            "批次 {batch_no} 开始｜批量拨款导入｜{}",
            path.display()
        )),
        operator,
    ) {
        report.warn(format!(
            "批次开始日志写入失败：{e}；请按批次号 {batch_no} 补记"
        ));
    }

    // 用工作表里的**绝对行号**，不能按 range 的相对序号算：
    // 若前几行是空的（用户把表头删了），calamine 的 Range 会从第一个有值的行开始，
    // 相对序号就会把数据行当成表头跳掉。
    let start_row = range.start().map(|(r, _)| r as usize).unwrap_or(0);
    for (i, row) in range.rows().enumerate() {
        let abs = start_row + i;
        if abs < FIRST_DATA_ROW {
            continue;
        }
        let line = abs + 1; // Excel 可见行号（从 1 起）
                            // 未填或填 0「本次金额」= 本行不导入、不算失败。模板会把全部合同预填一遍，
                            // 用户只填其中几行是正常用法。
        if cell_amount_skipped(row, 5) {
            report.skipped += 1;
            continue;
        }
        let name = cell(row, 0);
        if name.is_empty() {
            report.failures.push(ImportFailure {
                line,
                message: "合同名称为空，无法确定写到哪份合同".into(),
            });
            continue;
        }

        // 同一行内容已入账则跳过，不重复拨款；改过任一格才算新数据。
        let import_key = row_fingerprint(row, line);
        // R-70：查重失败必须显式报错，不能吞成「未入账」继续导入。
        let taken: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pay_payment WHERE import_key = ?1",
            [&import_key],
            |r| r.get(0),
        )?;
        if taken > 0 {
            report.duplicates += 1;
            continue;
        }

        let outcome = (|| -> std::result::Result<(), String> {
            let ids = by_name
                .get(name.as_str())
                .ok_or_else(|| format!("合同名称「{name}」在系统里没有，请核对全称"))?;
            if ids.len() > 1 {
                return Err(format!(
                    "合同名称「{name}」对应 {} 份合同，无法确定是哪一份，请改名后重试",
                    ids.len()
                ));
            }
            let amount = cell_money(row, 5)?;
            let date_raw = cell(row, 6);
            // 留空按当天（R-33，东八区口径 R-68 / C6）；其余仍必须是合法 ISO 日期
            let date = if date_raw.is_empty() {
                today_local_iso()
            } else {
                date_raw
            };
            if !is_iso_date(&date) {
                return Err(format!("开单日期「{date}」格式不对，应为 YYYY-MM-DD"));
            }
            let method = cell_pay_method(row, 8)?;
            let reason = cell(row, 7);
            let reason = if reason.is_empty() {
                "拨付进度款".to_string()
            } else {
                reason
            };

            let mut input = NewPayment::new(ids[0].to_string(), amount, date);
            input.pay_method = method;
            input.reason = Some(reason);
            input.import_key = Some(import_key.clone());
            service::register_payment(conn, input, operator).map_err(|e| e.to_string())?;
            Ok(())
        })();

        match outcome {
            Ok(()) => report.imported += 1,
            Err(message) => report.failures.push(ImportFailure { line, message }),
        }
    }

    // 批次结束事件：成功 / 跳过 / 重复 / 失败统计与文件名、批次号一起留痕。
    // 写失败不再静默吞掉，而是作为可见警告返回界面，并把批次号留给人工补记。
    if let Err(e) = db::insert_audit_log(
        conn,
        "import",
        &batch_no,
        Some(&file_label),
        "IMPORT_DONE",
        Some(&format!(
            "批次 {batch_no} 结束｜批量拨款导入｜成功 {} 笔，跳过 {} 行，重复 {} 行，失败 {} 行｜{}",
            report.imported,
            report.skipped,
            report.duplicates,
            report.failures.len(),
            path.display()
        )),
        operator,
    ) {
        report.warn(format!(
            "批次结果日志写入失败：{e}；请按批次号 {batch_no} 补记"
        ));
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctp_store::{db, seed};

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ctp_batch_{}_{}_{tag}.xlsx",
            std::process::id(),
            ctp_core::date::now_iso().replace([':', '-', 'T'], "")
        ))
    }

    fn conn_with_fixture() -> Connection {
        let mut c = db::open_in_memory().unwrap();
        db::init_schema(&mut c).unwrap();
        seed::seed_reference(&c, "t").unwrap();
        crate::fixture::seed_fixture(&mut c, "t").unwrap();
        c
    }

    fn fixture_rows(conn: &Connection) -> Vec<TemplateRow> {
        repo::summaries(conn)
            .unwrap()
            .iter()
            .map(TemplateRow::from_summary)
            .collect()
    }

    #[test]
    fn 模板可写出并带校验标记() {
        let conn = conn_with_fixture();
        let rows = fixture_rows(&conn);
        assert!(!rows.is_empty());
        let path = temp_path("write");
        let (n, cols) = write_template(&rows, &path, "2026-11-06T00:00:00Z").unwrap();
        assert_eq!(n, rows.len());
        assert_eq!(cols, 9);
        assert!(std::fs::metadata(&path).unwrap().len() > 1_000);
        // 用读取器回验：标记表 + 主表都在
        use calamine::Reader;
        let mut wb: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
        let names = wb.sheet_names().to_vec();
        assert!(names.contains(&TEMPLATE_SHEET.to_string()), "{names:?}");
        assert!(names.contains(&CHECK_SHEET.to_string()), "{names:?}");
        let check = wb.worksheet_range(CHECK_SHEET).unwrap();
        assert_eq!(check.get_value((0, 0)).map(cell_text).unwrap(), CHECK_MARK);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 标记不符时整份拒绝() {
        let mut conn = conn_with_fixture();
        let rows = fixture_rows(&conn);
        let path = temp_path("badmark");
        write_template(&rows, &path, "x").unwrap();

        // 把标记表首格改掉，再导
        {
            let mut wb = Workbook::new();
            let ws = wb.add_worksheet();
            ws.set_name(TEMPLATE_SHEET).unwrap();
            let bad = wb.add_worksheet();
            bad.set_name(CHECK_SHEET).unwrap();
            bad.write_string(0, 0, "SOMETHING-ELSE").unwrap();
            wb.save(&path).unwrap();
        }
        let err = import(&mut conn, &path, "t").unwrap_err();
        assert!(err.to_string().contains("校验标记不一致"), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 缺校验表时拒绝() {
        let mut conn = conn_with_fixture();
        let path = temp_path("nosheet");
        {
            let mut wb = Workbook::new();
            let ws = wb.add_worksheet();
            ws.set_name(TEMPLATE_SHEET).unwrap();
            wb.save(&path).unwrap();
        }
        let err = import(&mut conn, &path, "t").unwrap_err();
        assert!(err.to_string().contains("缺少校验工作表"), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 空行不算失败() {
        let mut conn = conn_with_fixture();
        let rows = fixture_rows(&conn);
        let path = temp_path("blank");
        write_template(&rows, &path, "x").unwrap();
        let report = import(&mut conn, &path, "批量导入").unwrap();
        assert_eq!(report.imported, 0, "没填本次金额就不该写入");
        assert!(report.skipped > 0, "空金额行应计入跳过");
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 金额为零跳过_留空字段按默认值写入() {
        let mut conn = conn_with_fixture();
        let rows = fixture_rows(&conn);
        let path = temp_path("defaults");
        let target = rows[0].contract_name.clone();
        {
            let mut wb = Workbook::new();
            let ws = wb.add_worksheet();
            ws.set_name(TEMPLATE_SHEET).unwrap();
            let r = FIRST_DATA_ROW as u32;
            // 第一行只填金额，日期 / 事由 / 支付方式全留空
            ws.write_string(r, 0, target.as_str()).unwrap();
            ws.write_number(r, 5, 100.0).unwrap();
            // 第二行金额填 0 → 跳过，不算失败
            ws.write_string(r + 1, 0, target.as_str()).unwrap();
            ws.write_number(r + 1, 5, 0.0).unwrap();
            let chk = wb.add_worksheet();
            chk.set_name(CHECK_SHEET).unwrap();
            chk.write_string(0, 0, CHECK_MARK).unwrap();
            wb.save(&path).unwrap();
        }

        let report = import(&mut conn, &path, "t").unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped, 1, "金额为 0 的行跳过且不算失败");
        assert!(report.failures.is_empty(), "{:?}", report.failures);

        let today = today_local_iso();
        let p = repo::list_payments(&conn, None)
            .unwrap()
            .into_iter()
            .find(|p| p.amount_cent.cent() == 10_000 && p.reason.as_deref() == Some("拨付进度款"))
            .expect("应写入带默认事由的单据");
        assert_eq!(p.pay_date, today, "开单日期留空按当天");
        assert_eq!(p.pay_method, PayMethod::Transfer, "支付方式留空按转账");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 填了金额的行会被写入且字段正确() {
        let mut conn = conn_with_fixture();
        let rows = fixture_rows(&conn);
        let before = repo::list_payments(&conn, None).unwrap().len();
        let target = rows[0].contract_name.clone();

        let path = temp_path("fill");
        {
            let mut wb = Workbook::new();
            let ws = wb.add_worksheet();
            ws.set_name(TEMPLATE_SHEET).unwrap();
            let r = FIRST_DATA_ROW as u32;
            ws.write_string(r, 0, target.as_str()).unwrap();
            ws.write_number(r, 5, 1888.5).unwrap();
            ws.write_string(r, 6, "2026-11-06").unwrap();
            ws.write_string(r, 7, "批量导入测试事由").unwrap();
            ws.write_string(r, 8, "现金").unwrap();
            let chk = wb.add_worksheet();
            chk.set_name(CHECK_SHEET).unwrap();
            chk.write_string(0, 0, CHECK_MARK).unwrap();
            wb.save(&path).unwrap();
        }

        let report = import(&mut conn, &path, "批量导入").unwrap();
        assert_eq!(report.failures, vec![], "{}", report.summary());
        assert_eq!(report.imported, 1);

        let after = repo::list_payments(&conn, None).unwrap();
        assert_eq!(after.len(), before + 1);
        let p = after
            .iter()
            .find(|p| p.reason.as_deref() == Some("批量导入测试事由"))
            .expect("应写入新单据");
        assert_eq!(p.amount_cent.cent(), 188_850, "1888.5 元 = 188850 分");
        assert_eq!(p.pay_method, PayMethod::Cash);
        assert_eq!(p.pay_date, "2026-11-06");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 合同名对不上时只报该行() {
        let mut conn = conn_with_fixture();
        let path = temp_path("badname");
        {
            let mut wb = Workbook::new();
            let ws = wb.add_worksheet();
            ws.set_name(TEMPLATE_SHEET).unwrap();
            let r = FIRST_DATA_ROW as u32;
            ws.write_string(r, 0, "根本不存在的合同").unwrap();
            ws.write_number(r, 5, 100.0).unwrap();
            ws.write_string(r, 6, "2026-11-06").unwrap();
            let chk = wb.add_worksheet();
            chk.set_name(CHECK_SHEET).unwrap();
            chk.write_string(0, 0, CHECK_MARK).unwrap();
            wb.save(&path).unwrap();
        }
        let report = import(&mut conn, &path, "t").unwrap();
        assert_eq!(report.imported, 0);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].line, FIRST_DATA_ROW + 1);
        assert!(
            report.failures[0].message.contains("没有"),
            "{:?}",
            report.failures
        );
        let _ = std::fs::remove_file(&path);
    }

    /// R-39：数值格也受单笔上限约束，不能靠「改成数值格式」绕过 parse_yuan。
    #[test]
    fn 数值金额同样受单笔上限约束() {
        let ok = vec![calamine::Data::Float(100.0)];
        assert_eq!(cell_money(&ok, 0).unwrap().cent(), 10_000);
        let too_big = vec![calamine::Data::Float(20_000_000_000.0)];
        assert!(cell_money(&too_big, 0).is_err(), "数值格超限应拒绝");
        let int_too_big = vec![calamine::Data::Int(i64::MAX)];
        assert!(cell_money(&int_too_big, 0).is_err(), "整数格溢出应拒绝");
        let nan = vec![calamine::Data::Float(f64::NAN)];
        assert!(cell_money(&nan, 0).is_err(), "非有限数应拒绝");
        let text_too_big = vec![calamine::Data::String("20000000000".into())];
        assert!(cell_money(&text_too_big, 0).is_err(), "文本格同样拒绝");
    }

    /// R-39：文件大小与行数上限在解压/逐行写入之前生效。
    #[test]
    fn 超大文件与超量行数被整份拒绝() {
        let mut conn = conn_with_fixture();
        let big = temp_path("bigfile");
        // 超过 10 MB 的文件，在 open_workbook 之前就被 metadata 检查挡下
        std::fs::write(&big, vec![0u8; (MAX_IMPORT_FILE_BYTES + 1) as usize]).unwrap();
        assert!(import(&mut conn, &big, "t").is_err());
        let _ = std::fs::remove_file(&big);

        let many = temp_path("manyrows");
        {
            let mut wb = Workbook::new();
            let ws = wb.add_worksheet();
            ws.set_name(TEMPLATE_SHEET).unwrap();
            for r in 0..=MAX_IMPORT_ROWS as u32 {
                ws.write_string(r, 0, "占位").unwrap();
            }
            let chk = wb.add_worksheet();
            chk.set_name(CHECK_SHEET).unwrap();
            chk.write_string(0, 0, CHECK_MARK).unwrap();
            wb.save(&many).unwrap();
        }
        let err = import(&mut conn, &many, "t").unwrap_err();
        assert!(err.to_string().contains("行上限"), "{err}");
        let _ = std::fs::remove_file(&many);
    }

    /// 落一份真实模板到临时目录，供肉眼核对版式（默认忽略）。
    #[test]
    #[ignore = "需要人工看版式时手动执行"]
    fn 落一份模板供肉眼核对() {
        let conn = conn_with_fixture();
        let rows = fixture_rows(&conn);
        let path = std::env::temp_dir().join("ctp-批量拨款模板_核对用.xlsx");
        let (n, _) = write_template(&rows, &path, &ctp_core::date::now_iso()).unwrap();
        println!("已写出 {} 行 → {}", n, path.display());
    }

    /// R-69：同一份表重复上传，已入账的行按 import_key 跳过，绝不重复拨款；
    /// 每次导入都有 START / DONE 两条批次日志，批次号能回溯到结果。
    #[test]
    fn 重复导入按幂等键跳过且批次日志可追溯() {
        let mut conn = conn_with_fixture();
        let rows = fixture_rows(&conn);
        let target = rows[0].contract_name.clone();
        let path = temp_path("idempotent");
        {
            let mut wb = Workbook::new();
            let ws = wb.add_worksheet();
            ws.set_name(TEMPLATE_SHEET).unwrap();
            let r = FIRST_DATA_ROW as u32;
            ws.write_string(r, 0, target.as_str()).unwrap();
            ws.write_number(r, 5, 100.0).unwrap();
            let chk = wb.add_worksheet();
            chk.set_name(CHECK_SHEET).unwrap();
            chk.write_string(0, 0, CHECK_MARK).unwrap();
            wb.save(&path).unwrap();
        }

        let first = import(&mut conn, &path, "t").unwrap();
        assert_eq!(first.imported, 1, "{:?}", first.failures);
        assert_eq!(first.duplicates, 0);
        assert!(!first.batch_no.is_empty(), "批次号必须返回给界面");
        assert!(first.warning.is_none(), "正常写日志不应有警告");

        let second = import(&mut conn, &path, "t").unwrap();
        assert_eq!(second.imported, 0, "同一行不能再入一次账");
        assert_eq!(second.duplicates, 1, "重复行应单独计数");
        assert!(second.failures.is_empty(), "{:?}", second.failures);

        let imported_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pay_payment WHERE import_key IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(imported_rows, 1, "两次导入只应产生一笔拨款");

        let starts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sys_audit_log WHERE action = 'IMPORT_START'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let dones: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sys_audit_log WHERE action = 'IMPORT_DONE'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((starts, dones), (2, 2), "每次导入各留起止两条事件");
        let hit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sys_audit_log WHERE target_id = ?1 AND action = 'IMPORT_DONE'",
                [&first.batch_no],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1, "批次号应能定位到该次导入的结果日志");
        let _ = std::fs::remove_file(&path);
    }

    /// 行指纹只取原始单元格：日期留空按当天展开，但隔天重传仍是同一行。
    #[test]
    fn 行指纹用原始值而非展开后的业务值() {
        let row = vec![
            calamine::Data::String("甲合同".into()),
            calamine::Data::Empty,
            calamine::Data::Float(100.0),
            calamine::Data::Empty,
        ];
        assert_eq!(row_fingerprint(&row, 4), row_fingerprint(&row, 4));
        assert_ne!(
            row_fingerprint(&row, 4),
            row_fingerprint(&row, 5),
            "不同行号必须是不同键"
        );
        let changed = vec![
            calamine::Data::String("甲合同".into()),
            calamine::Data::Empty,
            calamine::Data::Float(200.0),
            calamine::Data::Empty,
        ];
        assert_ne!(row_fingerprint(&row, 4), row_fingerprint(&changed, 4));
    }
}
