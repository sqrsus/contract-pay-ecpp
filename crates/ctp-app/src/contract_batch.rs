//! 批量合同：导出填表模板（xlsx）与导入回传。
//!
//! 与 `batch`（批量拨款）同构，共用同一套护栏：
//! * 隐藏工作表 `_CTP_CHECK` 带标记，标记不符整份拒绝（防止拿别的表改列名传上来）；
//! * 10 MB / 5000 行上限在解压与逐行写入之前生效；
//! * 逐行独立事务，失败行单独记行号，成功的行已经落库。
//!
//! 差别在于**语义方向相反**：拨款的模板按现有合同预填一批行、只让用户填后 4 列；
//! 合同的模板是「空表填新合同」，没有可预填的数据，因此除表头与说明外全是空白输入格，
//! 枚举列（合同类型 / 合同类别 / 计价方式 / 招标方式）带下拉，减少手打字面差异。
//!
//! **21 列 = 「新建合同」表单的全部字段**。刻意不含：
//! 合同编号（按「HT-年份-流水」自动生成）、审计价四列（走「登记审计价」留痕）、
//! 状态（走「设为生效」）、实际开竣工（走交工登记）、会计分类 / 预付款率 / 单体
//! （界面已无录入入口）。
//!
//! **导入一律草稿**：`activate_contract` 要求先有合同附件，Excel 带不了附件，
//! 所以导入只负责建档，生效与补附件仍在界面里做。
//!
//! **查重**：同名且同承包单位在库中已存在 → 该行失败。同一张表误传两次不会翻倍。

use crate::batch::{ImportFailure, ImportReport, MAX_IMPORT_FILE_BYTES, MAX_IMPORT_ROWS};
use crate::service::{self, NewContract};
use ctp_core::contract::{planned_end_for, BidMethod, ContractType, PricingMethod};
use ctp_core::date::is_iso_date;
use ctp_core::dict;
use ctp_core::money::Money;
use ctp_store::{repo, Result, StoreError};
use rusqlite::Connection;
use rust_xlsxwriter::{Color, DataValidation, Format, FormatAlign, FormatBorder, Workbook};
use std::collections::HashMap;
use std::path::Path;

/// 模板主工作表名。
pub const TEMPLATE_SHEET: &str = "批量合同";

/// 校验标记所在的工作表（隐藏）。与批量拨款同名：两种模板互相导入都会被标记挡下。
pub const CHECK_SHEET: &str = "_CTP_CHECK";

/// 校验标记；改它等于让所有旧模板失效，不要动。
pub const CHECK_MARK: &str = "CTP-BATCH-CONTRACT-V1";

// ---------------------------------------------------------------- 列定义

/// 列序号常量：解析与写模板共用，避免两处各写一遍魔法数字。
const C_NAME: usize = 0;
const C_PARTY_A: usize = 1;
const C_PROJECT: usize = 2;
const C_CONTRACTOR: usize = 3;
const C_LEADER: usize = 4;
const C_CONTACT: usize = 5;
const C_BID_METHOD: usize = 6;
const C_OPEN_BID: usize = 7;
const C_AGENT: usize = 8;
const C_START: usize = 9;
const C_DURATION: usize = 10;
const C_END: usize = 11;
const C_TYPE: usize = 12;
const C_CATEGORY: usize = 13;
const C_SIGN_DATE: usize = 14;
const C_AMOUNT: usize = 15;
const C_PRICING: usize = 16;
const C_RETENTION: usize = 17;
const C_DELIVERY: usize = 18;
const C_TERMS: usize = 19;
const C_REMARK: usize = 20;

/// 模板列标题（21 列），顺序与 `NewContract` 的录入顺序一致。
pub const HEADERS: [&str; 21] = [
    "合同名称*",
    "发包人（甲方）*",
    "所属项目*",
    "承包单位*",
    "项目负责人",
    "联系方式",
    "招标方式",
    "开标时间",
    "招标代理",
    "计划开工",
    "工期（日历天）",
    "计划交工",
    "合同类型",
    "合同类别*",
    "签订日期*",
    "合同金额（元）*",
    "计价方式",
    "质保金率（%）",
    "交工日期",
    "付款条款*",
    "备注",
];

/// 数据起始行（0-based）：第 1 行标题、第 2 行说明、第 3 行表头。
const FIRST_DATA_ROW: usize = 3;

/// 模板里预留的空白输入行数（只给格式与下拉，不写值）。
const TEMPLATE_BLANK_ROWS: u32 = 60;

/// 列宽（字符）。
const COL_WIDTHS: [f64; 21] = [
    30.0, 22.0, 20.0, 22.0, 12.0, 16.0, 11.0, 12.0, 20.0, 12.0, 12.0, 12.0, 10.0, 12.0, 12.0, 16.0,
    11.0, 12.0, 12.0, 34.0, 22.0,
];

fn xerr(e: impl std::fmt::Display) -> StoreError {
    StoreError::Export(e.to_string())
}

// ---------------------------------------------------------------- 写模板

/// 写出空白合同模板。返回列数。
///
/// `generated_at` 只写进说明行，便于事后确认「这份模板是哪天导出的」。
pub fn write_template(path: &Path, generated_at: &str) -> Result<usize> {
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
        // 必填列用浅黄底，和选填列区分开
        let header_req_fmt = Format::new()
            .set_bold()
            .set_font_color(Color::RGB(0xFFFFFF))
            .set_background_color(Color::RGB(0xB45309))
            .set_align(FormatAlign::Center)
            .set_text_wrap();

        let input_fmt = Format::new().set_border(FormatBorder::Thin);
        // 日期列一律文本格式：避免「2026-11-06」被 Excel 转成日期序列号
        let input_date_fmt = Format::new()
            .set_border(FormatBorder::Thin)
            .set_num_format("@");
        let input_money_fmt = Format::new()
            .set_border(FormatBorder::Thin)
            .set_num_format("#,##0.00")
            .set_align(FormatAlign::Right);
        let input_pct_fmt = Format::new()
            .set_border(FormatBorder::Thin)
            .set_num_format("0.00")
            .set_align(FormatAlign::Right);

        let last_col = (HEADERS.len() - 1) as u16;
        ws.merge_range(0, 0, 0, last_col, "批量合同导入模板", &title_fmt)
            .map_err(xerr)?;
        ws.merge_range(
            1,
            0,
            1,
            last_col,
            &format!(
                "带 * 为必填。日期一律填 YYYY-MM-DD 文本；合同金额填元（两位小数）；\
                 合同类型可填 工程/货物/服务（留空按工程）；\
                 合同类别必填，可填 工程款/设备款/材料款/监理费/设计费/检测费；\
                 计价方式可填 总价/单价/成本加酬金；招标方式可填 公开招标/政府采购/直接发包（留空按公开招标）；\
                 质保金率填百分数（如 3 表示 3%，留空按 3）；计划交工留空时按「计划开工 + 工期」推算；\
                 项目 / 承包单位 / 甲方 / 招标代理填名称即可，库里没有会自动建档；\
                 导入后一律为草稿，需在界面上传合同附件后才能「设为生效」。\
                 导出时间 {generated_at}"
            ),
            &note_fmt,
        )
        .map_err(xerr)?;

        let required = [
            C_NAME,
            C_PARTY_A,
            C_PROJECT,
            C_CONTRACTOR,
            C_CATEGORY,
            C_SIGN_DATE,
            C_AMOUNT,
            C_TERMS,
        ];
        for (ci, h) in HEADERS.iter().enumerate() {
            let fmt = if required.contains(&ci) {
                &header_req_fmt
            } else {
                &header_fmt
            };
            ws.write_string_with_format(2, ci as u16, *h, fmt)
                .map_err(xerr)?;
            ws.set_column_width(ci as u16, COL_WIDTHS[ci])
                .map_err(xerr)?;
        }
        ws.set_row_height(1, 46.0).map_err(xerr)?;
        ws.set_row_height(2, 28.0).map_err(xerr)?;

        let first = FIRST_DATA_ROW as u32;
        let last = first + TEMPLATE_BLANK_ROWS - 1;
        for row in first..=last {
            for ci in 0..HEADERS.len() {
                let fmt = match ci {
                    C_SIGN_DATE | C_OPEN_BID | C_START | C_END | C_DELIVERY => &input_date_fmt,
                    C_AMOUNT => &input_money_fmt,
                    C_RETENTION | C_DURATION => &input_pct_fmt,
                    _ => &input_fmt,
                };
                ws.write_blank(row, ci as u16, fmt).map_err(xerr)?;
            }
        }

        // 枚举列给下拉：字面不一致是这类导入最常见的失败原因
        for (col, list, hint) in [
            (
                C_TYPE,
                &["工程", "货物", "服务"][..],
                "合同类型：工程 / 货物 / 服务；留空按工程",
            ),
            (
                C_CATEGORY,
                &["工程款", "设备款", "材料款", "监理费", "设计费", "检测费"][..],
                "合同类别（必填）：工程款 / 设备款 / 材料款 / 监理费 / 设计费 / 检测费",
            ),
            (
                C_PRICING,
                &["总价", "单价", "成本加酬金"][..],
                "计价方式：总价 / 单价 / 成本加酬金；留空视为未选",
            ),
            (
                C_BID_METHOD,
                &["公开招标", "政府采购", "直接发包"][..],
                "招标方式：公开招标 / 政府采购 / 直接发包；留空按公开招标",
            ),
        ] {
            let dv = DataValidation::new()
                .allow_list_strings(list)
                .map_err(xerr)?
                .set_input_message(hint)
                .map_err(xerr)?;
            ws.add_data_validation(first, col as u16, last, col as u16, &dv)
                .map_err(xerr)?;
        }

        ws.set_freeze_panes(3, 0).map_err(xerr)?;
    }

    {
        // 隐藏的校验表：只放一个标记串（与批量拨款同名，两种模板不会互相被误认）
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
    Ok(HEADERS.len())
}

// ---------------------------------------------------------------- 单元格读取

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
        // 真日期格（用户从别的表粘进来）：转成 YYYY-MM-DD，省得让人重填
        calamine::Data::DateTime(dt) => {
            let (y, m, d, _, _, _, _) = dt.to_ymd_hms_milli();
            format!("{y:04}-{m:02}-{d:02}")
        }
        _ => String::new(),
    }
}

fn cell(row: &[calamine::Data], col: usize) -> String {
    row.get(col).map(cell_text).unwrap_or_default()
}

/// 整行是否什么都没有：模板预置 60 行空白，这些行不该计成「跳过」。
fn row_is_blank(row: &[calamine::Data]) -> bool {
    row.iter().all(|d| cell_text(d).is_empty())
}

/// 合同金额：必填、必须大于 0（与新建表单的 `amount_ok` 同一口径）。
fn cell_contract_money(row: &[calamine::Data], col: usize) -> std::result::Result<Money, String> {
    let raw = row.get(col);
    let money = match raw {
        Some(calamine::Data::Float(f)) => {
            if !f.is_finite() || *f < 0.0 {
                return Err("合同金额不是有效数字".into());
            }
            Money::from_cent((*f * 100.0).round() as i64)
        }
        Some(calamine::Data::Int(v)) => Money::from_cent(
            v.checked_mul(100)
                .ok_or_else(|| "合同金额过大".to_string())?,
        ),
        Some(calamine::Data::String(s)) => Money::parse_yuan(s).map_err(|e| e.to_string())?,
        _ => return Err("合同金额为空".into()),
    };
    if money.cent() <= 0 {
        return Err("合同金额必须大于 0".into());
    }
    Ok(money)
}

fn cell_duration(row: &[calamine::Data], col: usize) -> std::result::Result<i64, String> {
    let t = cell(row, col);
    if t.is_empty() {
        return Ok(0);
    }
    let v = t
        .parse::<i64>()
        .map_err(|_| format!("工期「{t}」不是整数，请填日历天数"))?;
    if v < 0 {
        return Err("工期不能为负".into());
    }
    Ok(v)
}

/// 质保金率：百分数文本（`3` / `3.5` / `3%`）→ 万分比；留空按 3%（与新建表单默认一致）。
fn cell_retention_bp(row: &[calamine::Data], col: usize) -> std::result::Result<i32, String> {
    let t = cell(row, col);
    let t = t.trim().trim_end_matches('%').trim();
    if t.is_empty() {
        return Ok(300);
    }
    let v: f64 = t
        .parse()
        .map_err(|_| format!("质保金率「{t}」不是数字，请填百分数（如 3 表示 3%）"))?;
    if !(0.0..=100.0).contains(&v) {
        return Err("质保金率应在 0 到 100 之间".into());
    }
    Ok((v * 100.0).round() as i32)
}

fn opt_date(
    row: &[calamine::Data],
    col: usize,
    label: &str,
) -> std::result::Result<Option<String>, String> {
    let t = cell(row, col);
    if t.is_empty() {
        return Ok(None);
    }
    if !is_iso_date(&t) {
        return Err(format!("{label}「{t}」格式不对，应为 YYYY-MM-DD"));
    }
    Ok(Some(t))
}

/// 合同类型：中文标签或英文码都认；留空按工程（与新建表单的默认选中项一致）。
fn cell_contract_type(
    row: &[calamine::Data],
    col: usize,
) -> std::result::Result<ContractType, String> {
    let t = cell(row, col);
    if t.is_empty() {
        return Ok(ContractType::Engineering);
    }
    for ty in ContractType::ALL {
        if t == ty.label() || t.eq_ignore_ascii_case(ty.code()) {
            return Ok(ty);
        }
    }
    Err(format!("合同类型「{t}」无法识别，可填 工程/货物/服务"))
}

/// 合同类别：必须命中字典（`sys_dict_item:CONTRACT_CATEGORY`），否则该行失败。
fn cell_category(row: &[calamine::Data], col: usize) -> std::result::Result<String, String> {
    let t = cell(row, col);
    if t.is_empty() {
        return Err("合同类别为空，可填 工程款/设备款/材料款/监理费/设计费/检测费".into());
    }
    for (code, name) in dict::CONTRACT_CATEGORIES {
        if t == *name || t.eq_ignore_ascii_case(code) {
            return Ok((*code).to_string());
        }
    }
    Err(format!(
        "合同类别「{t}」不在字典里，可填 工程款/设备款/材料款/监理费/设计费/检测费"
    ))
}

fn cell_pricing(
    row: &[calamine::Data],
    col: usize,
) -> std::result::Result<Option<PricingMethod>, String> {
    let t = cell(row, col);
    if t.is_empty() {
        return Ok(None);
    }
    for m in [
        PricingMethod::LumpSum,
        PricingMethod::UnitPrice,
        PricingMethod::CostPlus,
    ] {
        if t == m.label() || t.eq_ignore_ascii_case(m.code()) {
            return Ok(Some(m));
        }
    }
    Err(format!(
        "计价方式「{t}」无法识别，可填 总价/单价/成本加酬金"
    ))
}

fn cell_bid_method(
    row: &[calamine::Data],
    col: usize,
) -> std::result::Result<Option<BidMethod>, String> {
    let t = cell(row, col);
    if t.is_empty() {
        // R-57：招标方式不再有「未选」，留空按公开招标（与新建表单默认值一致）。
        return Ok(Some(BidMethod::Open));
    }
    for m in [BidMethod::Open, BidMethod::Government, BidMethod::Direct] {
        if t == m.label() || t.eq_ignore_ascii_case(m.code()) {
            return Ok(Some(m));
        }
    }
    Err(format!(
        "招标方式「{t}」无法识别，可填 公开招标/政府采购/直接发包"
    ))
}

// ---------------------------------------------------------------- 导入

/// 查重键：与 `ensure_contractor` 同一套「编码 名称」规则，写成 `ORG-1 某单位`
/// 与 `某单位` 视为同一家，不会因为多打一个编码就重复建档。
fn dup_key(name: &str, unit: &str) -> (String, String) {
    let unit = unit.trim();
    let unit = match service::split_code_name(unit) {
        Some((head, tail)) if service::looks_like_code(head) => tail,
        _ => unit,
    };
    (name.trim().to_string(), unit.to_string())
}

/// 读取并导入。整份先过校验标记，再逐行写入。
///
/// 返回 `Err` 表示整份文件不可用；行级问题放在 `ImportReport::failures` 里，不影响其他行。
pub fn import(conn: &mut Connection, path: &Path, operator: &str) -> Result<ImportReport> {
    use calamine::Reader;

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
            "缺少校验工作表：这不是本系统导出的模板，请用「导出批量合同模板」重新导出",
        ));
    }
    let check = wb
        .worksheet_range(CHECK_SHEET)
        .map_err(|e| xerr(format!("读取校验表失败：{e}")))?;
    let mark = check.get_value((0, 0)).map(cell_text).unwrap_or_default();
    if mark != CHECK_MARK {
        return Err(xerr(format!(
            "校验标记不一致（读到「{mark}」，应为「{CHECK_MARK}」）；\
             这可能是批量拨款模板，请用「导出批量合同模板」重新导出"
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

    // 3) 现有「名称 + 承包单位」索引：重复导入同一张表不会翻倍
    let mut existing: HashMap<(String, String), usize> = HashMap::new();
    for s in repo::summaries(conn)? {
        *existing
            .entry((s.contract_name.clone(), s.contractor_name.clone()))
            .or_insert(0) += 1;
    }

    let mut report = ImportReport::default();
    let start_row = range.start().map(|(r, _)| r as usize).unwrap_or(0);
    for (i, row) in range.rows().enumerate() {
        let abs = start_row + i;
        if abs < FIRST_DATA_ROW {
            continue;
        }
        // 模板预置的空白行直接忽略，不计入统计
        if row_is_blank(row) {
            continue;
        }
        let line = abs + 1; // Excel 可见行号（从 1 起）

        let outcome = (|| -> std::result::Result<(), String> {
            let name = cell(row, C_NAME);
            if name.is_empty() {
                return Err("合同名称为空，无法建档".into());
            }
            let party_a = cell(row, C_PARTY_A);
            if party_a.is_empty() {
                return Err("发包人（甲方）为空".into());
            }
            let project = cell(row, C_PROJECT);
            if project.is_empty() {
                return Err("所属项目为空".into());
            }
            let contractor = cell(row, C_CONTRACTOR);
            if contractor.is_empty() {
                return Err("承包单位为空".into());
            }
            let terms = cell(row, C_TERMS);
            if terms.is_empty() {
                return Err("付款条款为空（合同登记的必填项）".into());
            }
            let category_code = cell_category(row, C_CATEGORY)?;
            let contract_type = cell_contract_type(row, C_TYPE)?;
            let amount = cell_contract_money(row, C_AMOUNT)?;
            let sign_date = cell(row, C_SIGN_DATE);
            if sign_date.is_empty() {
                return Err("签订日期为空".into());
            }
            if !is_iso_date(&sign_date) {
                return Err(format!("签订日期「{sign_date}」格式不对，应为 YYYY-MM-DD"));
            }
            let open_bid_date = opt_date(row, C_OPEN_BID, "开标时间")?;
            let planned_start = opt_date(row, C_START, "计划开工")?;
            let delivery_date = opt_date(row, C_DELIVERY, "交工日期")?;
            let duration = cell_duration(row, C_DURATION)?;
            let planned_end = match opt_date(row, C_END, "计划交工")? {
                Some(d) => Some(d),
                // 与新建表单一致：留空时按「开工 + 工期」推算（含开工当日）
                None => planned_start
                    .as_deref()
                    .and_then(|s| planned_end_for(s, duration)),
            };
            let pricing = cell_pricing(row, C_PRICING)?;
            let bid_method = cell_bid_method(row, C_BID_METHOD)?;
            let retention_bp = cell_retention_bp(row, C_RETENTION)?;

            let key = dup_key(&name, &contractor);
            if existing.get(&key).is_some_and(|n| *n > 0) {
                return Err(format!(
                    "「{name}」与同一承包单位的合同已存在，疑似重复导入；请改名或跳过该行"
                ));
            }

            let leader = non_empty(cell(row, C_LEADER));
            let contact = non_empty(cell(row, C_CONTACT));
            let agent = non_empty(cell(row, C_AGENT));
            let remark = non_empty(cell(row, C_REMARK));

            service::create_contract(
                conn,
                NewContract {
                    project_id: String::new(),
                    project_text: Some(project),
                    contractor_id: String::new(),
                    contractor_text: Some(contractor),
                    contract_name: name,
                    contract_type,
                    contract_category_code: category_code,
                    category_id: None,
                    party_a,
                    building_id: None,
                    sign_date,
                    original_amount: amount,
                    duration_days: duration,
                    planned_start,
                    planned_end,
                    delivery_date,
                    pricing_method: pricing,
                    retention_rate_bp: retention_bp,
                    advance_rate_bp: 0,
                    payment_terms: terms,
                    remark,
                    bid_method,
                    open_bid_date,
                    project_leader: leader,
                    contact_info: contact,
                    bid_agent_text: agent,
                },
                operator,
            )
            .map_err(|e| e.to_string())?;
            // 记进索引：同一张表里出现两行同名同单位，第二行也要算重复
            *existing.entry(key).or_insert(0) += 1;
            Ok(())
        })();

        match outcome {
            Ok(()) => report.imported += 1,
            Err(message) => report.failures.push(ImportFailure { line, message }),
        }
    }

    Ok(report)
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctp_store::{db, seed};

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ctp_contract_batch_{}_{}_{tag}.xlsx",
            std::process::id(),
            ctp_core::date::now_iso().replace([':', '-', 'T'], "")
        ))
    }

    fn conn_with_fixture() -> Connection {
        let mut c = db::open_in_memory().unwrap();
        db::migrate(&mut c).unwrap();
        seed::seed_reference(&c, "t").unwrap();
        crate::fixture::seed_fixture(&mut c, "t").unwrap();
        c
    }

    /// 一份只有表头 + 一行数据的模板（带校验表），行内容由调用方给。
    fn workbook_with_row(path: &Path, cells: &[(usize, &str)]) {
        let mut wb = Workbook::new();
        {
            let ws = wb.add_worksheet();
            ws.set_name(TEMPLATE_SHEET).unwrap();
            for (col, v) in cells {
                ws.write_string(FIRST_DATA_ROW as u32, *col as u16, *v)
                    .unwrap();
            }
        }
        let chk = wb.add_worksheet();
        chk.set_name(CHECK_SHEET).unwrap();
        chk.write_string(0, 0, CHECK_MARK).unwrap();
        wb.save(path).unwrap();
    }

    fn full_row() -> Vec<(usize, &'static str)> {
        vec![
            (C_NAME, "批量导入测试合同"),
            (C_PARTY_A, "某建设单位"),
            (C_PROJECT, "批量导入测试项目"),
            (C_CONTRACTOR, "某施工单位"),
            (C_LEADER, "张三"),
            (C_CONTACT, "13800000000"),
            (C_BID_METHOD, "公开招标"),
            (C_OPEN_BID, "2026-01-05"),
            (C_AGENT, "某招标代理"),
            (C_START, "2026-03-01"),
            (C_DURATION, "90"),
            (C_TYPE, "工程"),
            (C_CATEGORY, "工程款"),
            (C_SIGN_DATE, "2026-02-10"),
            (C_AMOUNT, "1234567.89"),
            (C_PRICING, "单价"),
            (C_RETENTION, "3.5"),
            (C_DELIVERY, "2026-08-01"),
            (C_TERMS, "按月计量支付至 80%"),
            (C_REMARK, "批量导入"),
        ]
    }

    #[test]
    fn 模板写出二十一列并带校验标记() {
        let path = temp_path("write");
        let cols = write_template(&path, "2026-11-09T00:00:00Z").unwrap();
        assert_eq!(cols, 21);
        assert_eq!(HEADERS.len(), 21);
        assert!(std::fs::metadata(&path).unwrap().len() > 1_000);

        use calamine::Reader;
        let mut wb: calamine::Xlsx<_> = calamine::open_workbook(&path).unwrap();
        let names = wb.sheet_names().to_vec();
        assert!(names.contains(&TEMPLATE_SHEET.to_string()), "{names:?}");
        assert!(names.contains(&CHECK_SHEET.to_string()), "{names:?}");
        let check = wb.worksheet_range(CHECK_SHEET).unwrap();
        assert_eq!(check.get_value((0, 0)).map(cell_text).unwrap(), CHECK_MARK);

        // 表头逐列核对：列顺序就是导入解析顺序，错一列整表就错位
        let range = wb.worksheet_range(TEMPLATE_SHEET).unwrap();
        let rows: Vec<&[calamine::Data]> = range.rows().collect();
        let header_row: Vec<String> = (0..21).map(|c| cell(rows[2], c)).collect();
        assert_eq!(header_row, HEADERS.to_vec());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 用拨款模板导合同会被标记挡下() {
        let mut conn = conn_with_fixture();
        let path = temp_path("paytpl");
        {
            let mut wb = Workbook::new();
            wb.add_worksheet().set_name(TEMPLATE_SHEET).unwrap();
            let chk = wb.add_worksheet();
            chk.set_name(CHECK_SHEET).unwrap();
            chk.write_string(0, 0, crate::batch::CHECK_MARK).unwrap();
            wb.save(&path).unwrap();
        }
        let err = import(&mut conn, &path, "t").unwrap_err();
        assert!(err.to_string().contains("校验标记不一致"), "{err}");
        assert!(err.to_string().contains("批量拨款模板"), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 完整一行导入后字段全部落库() {
        let mut conn = conn_with_fixture();
        let path = temp_path("full");
        workbook_with_row(&path, &full_row());

        let report = import(&mut conn, &path, "批量导入").unwrap();
        assert_eq!(report.failures, vec![], "{}", report.summary());
        assert_eq!(report.imported, 1);

        let c = repo::summaries(&conn)
            .unwrap()
            .into_iter()
            .find(|s| s.contract_name == "批量导入测试合同")
            .expect("应写入新合同");
        assert_eq!(c.contractor_name, "某施工单位");
        assert_eq!(c.total_amount.fixed_yuan(), "1234567.89");
        assert!(c.contract_no.starts_with("HT-"), "{}", c.contract_no);

        let full = repo::get_contract(&conn, &c.contract_id).unwrap();
        assert_eq!(full.contract_type, ContractType::Engineering);
        assert_eq!(full.contract_category_code, "GC");
        assert_eq!(full.party_a, "某建设单位");
        assert_eq!(full.sign_date, "2026-02-10");
        assert_eq!(full.duration_days, 90);
        assert_eq!(full.planned_start.as_deref(), Some("2026-03-01"));
        assert_eq!(
            full.planned_end.as_deref(),
            Some("2026-05-29"),
            "计划交工留空时按开工 + 工期推算"
        );
        assert_eq!(full.delivery_date.as_deref(), Some("2026-08-01"));
        assert_eq!(full.pricing_method, Some(PricingMethod::UnitPrice));
        assert_eq!(full.bid_method, Some(BidMethod::Open));
        assert_eq!(full.retention_rate_bp, 350);
        assert_eq!(full.advance_rate_bp, 0);
        assert_eq!(full.project_leader.as_deref(), Some("张三"));
        assert_eq!(full.contact_info.as_deref(), Some("13800000000"));
        assert_eq!(full.open_bid_date.as_deref(), Some("2026-01-05"));
        assert_eq!(full.payment_terms, "按月计量支付至 80%");
        assert_eq!(full.status, ctp_core::contract::ContractStatus::Draft);
        // 甲方与代理都进了单位档案（角色不同，不混进承包单位联想）
        assert!(repo::find_contractor_by_name(&conn, "某招标代理")
            .unwrap()
            .is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 同名同承包单位重复导入只成功一次() {
        let mut conn = conn_with_fixture();
        let path = temp_path("dup");
        workbook_with_row(&path, &full_row());
        assert_eq!(import(&mut conn, &path, "t").unwrap().imported, 1);
        let _ = std::fs::remove_file(&path);

        // 第二次同一份表：该行失败，库里仍只有一份
        let path2 = temp_path("dup2");
        workbook_with_row(&path2, &full_row());
        let report = import(&mut conn, &path2, "t").unwrap();
        assert_eq!(report.imported, 0);
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].message.contains("疑似重复导入"));
        let count = repo::summaries(&conn)
            .unwrap()
            .iter()
            .filter(|s| s.contract_name == "批量导入测试合同")
            .count();
        assert_eq!(count, 1, "不能因为重复导入变成两份");
        let _ = std::fs::remove_file(&path2);
    }

    #[test]
    fn 必填缺失与枚举非法只报该行() {
        let mut conn = conn_with_fixture();
        let path = temp_path("bad");
        {
            let mut wb = Workbook::new();
            {
                let ws = wb.add_worksheet();
                ws.set_name(TEMPLATE_SHEET).unwrap();
                let r = FIRST_DATA_ROW as u32;
                // 第 1 行：缺合同名称
                ws.write_string(r, C_PROJECT as u16, "某项目").unwrap();
                // 第 2 行：合同类别不在字典里
                for (col, v) in full_row() {
                    let v = if col == C_CATEGORY {
                        "不存在的类别"
                    } else {
                        v
                    };
                    ws.write_string(r + 1, col as u16, v).unwrap();
                }
                // 第 3 行：金额为 0
                for (col, v) in full_row() {
                    let v = if col == C_AMOUNT { "0" } else { v };
                    ws.write_string(r + 2, col as u16, v).unwrap();
                }
            }
            let chk = wb.add_worksheet();
            chk.set_name(CHECK_SHEET).unwrap();
            chk.write_string(0, 0, CHECK_MARK).unwrap();
            wb.save(&path).unwrap();
        }

        let report = import(&mut conn, &path, "t").unwrap();
        assert_eq!(report.imported, 0, "{}", report.summary());
        assert_eq!(report.failures.len(), 3, "{:?}", report.failures);
        assert!(report.failures[0].message.contains("合同名称为空"));
        assert!(report.failures[1].message.contains("合同类别"));
        assert!(report.failures[2].message.contains("必须大于 0"));
        assert_eq!(report.failures[0].line, FIRST_DATA_ROW + 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 空白行不计入统计() {
        let mut conn = conn_with_fixture();
        let path = temp_path("blank");
        // 与真实模板一样：只有表头与空白输入行
        write_template(&path, "x").unwrap();
        let report = import(&mut conn, &path, "t").unwrap();
        assert_eq!(report.imported, 0);
        assert_eq!(report.skipped, 0, "预置空白行不该报「跳过 60 行」");
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 百分数与工期解析容错() {
        let row = vec![calamine::Data::String("3%".into())];
        assert_eq!(cell_retention_bp(&row, 0).unwrap(), 300);
        let row = vec![calamine::Data::Float(3.5)];
        assert_eq!(cell_retention_bp(&row, 0).unwrap(), 350);
        let row = vec![calamine::Data::Empty];
        assert_eq!(cell_retention_bp(&row, 0).unwrap(), 300, "留空按 3%");

        let row = vec![calamine::Data::String("90".into())];
        assert_eq!(cell_duration(&row, 0).unwrap(), 90);
        let row = vec![calamine::Data::String("九十".into())];
        assert!(cell_duration(&row, 0).is_err());
    }

    /// 落一份模板到临时目录，供人工核对版式（默认忽略）。
    #[test]
    #[ignore = "需要人工看版式时手动执行"]
    fn 落一份模板供肉眼核对() {
        let path = std::env::temp_dir().join("ctp-批量合同模板_核对用.xlsx");
        let cols = write_template(&path, &ctp_core::date::now_iso()).unwrap();
        println!("已写出 {cols} 列 → {}", path.display());
    }
}
