//! 弹窗表单：新建合同、登记审计价、编辑合同金额、作废确认。
//!
//! 所有弹窗都复用同一套「卡片式」外观，并在提交按钮上做可用性判断：
//! 必填未填或数值非法时按钮置灰，避免用户点了才看到报错。

use crate::theme::Palette;
use crate::widgets::{self, Tone};
use crate::App;
use ctp_app::report::{ReportColumnError, ReportResult};
use ctp_app::service::{self, EditContract, NewContract, NewPayment};
use ctp_core::attachment::{Attachment, AttachmentKind};
use ctp_core::contract::{BidMethod, Contract, ContractType, PricingMethod};
use ctp_core::date::{is_iso_date, today_iso};
use ctp_core::dict;
use ctp_core::money::Money;
use ctp_core::payment::PayMethod;
use ctp_core::project::ContractorRole;
use ctp_core::rate::Rate;
use ctp_store::repo;
use eframe::egui::{self, RichText};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    None,
    NewContract,
    AuditPrice,
    /// 项目交工：登记交工日期
    DeliveryDate,
    EditAmount,
    PaymentDetail,
    /// 付款打印预览：按真实版面渲染的整页位图，确认后直接送打印
    PrintPreview,
    /// 合同附件管理（从合同详情卡的「附件 n/4」进来）
    Attachments,
    /// 附件材料打印（预留：中标通知书、审计报告等）
    MaterialPrint,
    /// 合同详情只读弹窗（点击合同编号）
    ContractDetail,
    /// 批量拨款导入结果（R-33）：常驻、真模态，仅「关闭」可退出
    ImportResult,
    /// 合同台账导出列选择（R-46）：导出前勾选字段并调整顺序
    ExportColumns,
    /// 合同信息修改（R-50）：右侧栏第 5 个按钮进入，只改主档字段
    EditContract,
}

/// 导入结果弹窗的数据：只保留统计摘要与失败条目，不逐行列明细（R-33 确认口径）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportResultView {
    pub imported: usize,
    pub skipped: usize,
    /// （Excel 可见行号，失败原因）
    pub failures: Vec<(usize, String)>,
    /// 整份拒绝的原因（文件打不开 / 缺校验表 / 标记不符 / 无权限等）
    pub rejected: Option<String>,
}

/// 关闭导入结果弹窗：把弹窗状态与结果数据一起清空。
///
/// 抽成不依赖渲染上下文的纯状态变更，「关闭」按钮与登出两条路径共用，
/// 也便于单测锁定「两处都清空」——避免只把 `dialog` 置回 `None`，
/// 却把上一次的统计摘要留在 `import_result` 里造成下次打开显示脏数据。
pub fn dismiss_import_result(dialog: &mut Dialog, result: &mut Option<ImportResultView>) {
    *dialog = Dialog::None;
    *result = None;
}

// ---------------------------------------------------------------- 导出列选择（R-46）

/// 一个可选字段。`label` 是 Excel 里实际的表头文字（金额列含「（元）」），
/// 让「选择结果」与「生成的列名」逐字一致；`key` 用于投影 `ReportResult`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportColumnChoice {
    pub key: &'static str,
    pub label: String,
    pub selected: bool,
}

/// 合同台账导出列选择弹窗的状态。
///
/// `result` 是点「导出 Excel」那一刻按当前筛选 / 排序构造的完整 9 列结果快照：
/// 弹窗期间用户改不了后台列表，因此导出的数据范围与列表当时所见严格一致。
#[derive(Clone, Debug, PartialEq)]
pub struct ExportColumnsView {
    pub result: ReportResult,
    pub choices: Vec<ExportColumnChoice>,
    pub error: Option<String>,
}

impl ExportColumnsView {
    pub fn new(result: ReportResult) -> Self {
        let choices = result
            .columns
            .iter()
            .map(|c| ExportColumnChoice {
                key: c.key,
                label: c.excel_title(),
                selected: true,
            })
            .collect();
        ExportColumnsView {
            result,
            choices,
            error: None,
        }
    }

    /// 勾选的列 key，按列表当前顺序（即 Excel 列顺序）。
    pub fn selected_keys(&self) -> Vec<&'static str> {
        self.choices
            .iter()
            .filter(|c| c.selected)
            .map(|c| c.key)
            .collect()
    }

    pub fn set_all(&mut self, on: bool) {
        for c in &mut self.choices {
            c.selected = on;
        }
        self.error = None;
    }

    /// 恢复默认：全选 + 原始列序。
    pub fn reset(&mut self) {
        let keys: Vec<&'static str> = self.result.columns.iter().map(|c| c.key).collect();
        self.choices
            .sort_by_key(|c| keys.iter().position(|k| *k == c.key).unwrap_or(usize::MAX));
        self.set_all(true);
    }

    pub fn move_up(&mut self, i: usize) {
        if i > 0 && i < self.choices.len() {
            self.choices.swap(i - 1, i);
        }
    }

    pub fn move_down(&mut self, i: usize) {
        if i + 1 < self.choices.len() {
            self.choices.swap(i, i + 1);
        }
    }

    /// 确认导出：按勾选顺序投影出一份只含所选列的 `ReportResult`。
    pub fn confirm(&self) -> Result<ReportResult, ReportColumnError> {
        self.result.select_columns(&self.selected_keys())
    }
}

/// 关闭导出列选择：与登出共用，避免只清弹窗不清数据导致下次带出旧选择。
pub fn dismiss_export_columns(dialog: &mut Dialog, view: &mut Option<ExportColumnsView>) {
    *dialog = Dialog::None;
    *view = None;
}

// ---------------------------------------------------------------- 打印失败提示（R-42）

/// 打印失败的三个来源：准备打印数据、真正出纸、生成预览画面。
///
/// 底层抛的都是同一个 `PrintError`，但同一个错误在不同来源下给用户的解释与建议
/// 并不相同，所以分类时必须带上来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintStage {
    Prepare,
    Print,
    Preview,
}

/// 打印失败弹窗的全部内容。
///
/// `reason` / `solution` 是给用户看的人话，`technical` 是原始错误串，
/// 只作为底部小字「技术信息」附在弹窗里，便于管理员排查。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintFailureView {
    /// 固定标题，所有失败一致。
    pub title: &'static str,
    /// 短标签，显示在标题右侧的徽标里。
    pub kind: &'static str,
    pub reason: String,
    pub solution: String,
    pub technical: String,
}

const PRINT_FAILURE_TITLE: &str = "打印失败";

fn failure_view(
    kind: &'static str,
    reason: impl Into<String>,
    solution: impl Into<String>,
    technical: String,
) -> PrintFailureView {
    PrintFailureView {
        title: PRINT_FAILURE_TITLE,
        kind,
        reason: reason.into(),
        solution: solution.into(),
        technical,
    }
}

/// 未知原因 / 拿不到原因时的统一兜底：所有未归类错误返回完全相同的一份内容，
/// 避免同一个故障在不同入口出现不同说法。
fn unknown_failure(technical: String) -> PrintFailureView {
    failure_view(
        "原因未获取",
        "未获取到本次打印失败的具体原因。",
        "请检查打印机是否就绪以及 Windows 打印队列，然后重试；若持续失败，请联系管理员。",
        technical,
    )
}

/// 把底层 `PrintError` 翻译成用户能看懂、能照做的提示。
///
/// 只在 GUI 层做映射，不改动 `ctp-app` 的公共 API：出纸 / 取数 / 预览三条链路
/// 继续抛原来的错误，其他模块零影响。
pub fn classify_print_failure(
    stage: PrintStage,
    e: &ctp_app::print::PrintError,
) -> PrintFailureView {
    let technical = e.to_string();
    match stage {
        PrintStage::Prepare => prepare_failure(e, technical),
        PrintStage::Preview => failure_view(
            "预览渲染失败",
            "预览画面未能生成；本次打印数据已经准备完成，出纸链路不受影响。",
            "请点击「关闭」返回预览窗后直接点「打印」出纸；若反复出现，请重启程序后再试。",
            technical,
        ),
        PrintStage::Print => print_reason(e, technical),
    }
}

fn prepare_failure(e: &ctp_app::print::PrintError, technical: String) -> PrintFailureView {
    use ctp_app::print::PrintError as E;
    let detail = match e {
        E::Voided(no) => format!("所选拨款单 {no} 已作废，按口径不允许打印。"),
        E::MissingField(f) => format!("打印所需的「{f}」字段缺失，无法生成完整版面。"),
        E::NothingSelected => "没有选中任何拨款单，无法生成打印数据。".into(),
        E::Store(err) => format!("读取打印数据失败：{err}"),
        E::Print(msg) => format!("打印数据准备未完成：{msg}"),
        E::Cancelled => "打印数据准备被取消。".into(),
    };
    failure_view(
        "打印数据准备失败",
        format!("打印数据准备失败，未向打印机发送任何内容。{detail}"),
        "请刷新页面后重新选择拨款单再打印；若仍失败，请联系管理员检查单据数据。",
        technical,
    )
}

/// 出纸阶段的 7 类固定消息 → 用户文案。顺序即优先级：
/// 先判纸张方向 / 对话框 / 作业启动，再判带页码的中途失败，最后判作业结束。
fn print_reason(e: &ctp_app::print::PrintError, technical: String) -> PrintFailureView {
    use ctp_app::print::PrintError as E;
    let E::Print(msg) = e else {
        // 出纸阶段理论上只会拿到 Print(..)；万一拿到别的变体也不能把原因吞掉。
        return failure_view(
            "打印数据准备失败",
            format!("打印未完成，未向打印机发送任何内容：{technical}"),
            "请刷新页面后重新选择单据再打印；若仍失败，请联系管理员。",
            technical,
        );
    };
    if msg.contains("纵向") || msg.contains("纸型") {
        return failure_view(
            "纸张方向不符",
            "打印机当前设置为纵向，A4 横向的单据会被裁掉右半边，程序已拦下本次打印。",
            "在系统打印对话框的「首选项」里把纸张方向改为「横向」（A4），然后重新点击「打印」。",
            technical,
        );
    }
    if msg.contains("打印对话框") || msg.contains("没有返回打印机句柄") {
        return failure_view(
            "打印对话框异常",
            "系统打印对话框未能正常打开，或没有返回可用的打印机句柄。",
            "请确认已安装打印机并设为默认打印机；关闭后重新打开程序再试。",
            technical,
        );
    }
    if msg.contains("无法开始打印作业") {
        return failure_view(
            "打印机未就绪",
            "打印作业没能启动，通常是打印机未开机、断线、缺纸或队列卡住。",
            "请检查打印机开机与连线、补足纸张、清空打印队列后重新打印。",
            technical,
        );
    }
    if msg.starts_with("第 ") && (msg.contains("页开始失败") || msg.contains("页输出失败"))
    {
        let page: String = msg.chars().filter(|c| c.is_ascii_digit()).collect();
        return failure_view(
            "出纸中断",
            format!("第 {page} 页输出中断，多为缺纸或卡纸导致；本次作业已中止。"),
            "请处理卡纸 / 补足纸张后重新打印；已经输出的纸张请勿重复使用，避免重复报销。",
            technical,
        );
    }
    if msg.contains("打印作业结束失败") || msg.contains("释放打印机句柄失败") {
        return failure_view(
            "作业未正常结束",
            "打印作业未能正常提交，纸张可能未完整输出。",
            "请到 Windows 打印队列确认是否有卡住的任务，清理后重试；重打前先确认是否已经出纸，避免重复出纸。",
            technical,
        );
    }
    unknown_failure(technical)
}

/// 只有 `Err` 才生成失败弹窗；`Ok(0)`（用户取消）与 `Ok(n)`（成功）一律返回 None。
///
/// 用泛型接住 `Result<usize, _>`（出纸）与 `Result<Vec<Voucher>, _>`（取数），
/// 让「成功 / 取消不弹窗」这条边界由类型而不是调用方自觉来保证。
pub fn failure_for_outcome<T>(
    stage: PrintStage,
    outcome: &std::result::Result<T, ctp_app::print::PrintError>,
) -> Option<PrintFailureView> {
    outcome
        .as_ref()
        .err()
        .map(|e| classify_print_failure(stage, e))
}

/// 同一时间只允许一个打印失败弹窗：已有未关闭的弹窗时忽略后续失败，
/// 保留首次原因，不重复弹出、不互相覆盖。返回是否真正展示了新弹窗。
pub fn offer_print_failure(slot: &mut Option<PrintFailureView>, v: PrintFailureView) -> bool {
    if slot.is_some() {
        return false;
    }
    *slot = Some(v);
    true
}

/// 关闭打印失败弹窗：只清失败提示，预览单据 / 页码 / 位图一律保留，
/// 让用户关掉提示就能直接再点「打印」。
pub fn dismiss_print_failure(view: &mut Option<PrintFailureView>) {
    *view = None;
}

// ---------------------------------------------------------------- 打印成功提示（R-44）

/// 打印成功提示的自动关闭时长（秒）。
pub const PRINT_SUCCESS_SECS: u64 = 5;
const PRINT_SUCCESS_TITLE: &str = "打印已提交";

/// 打印成功弹窗的全部内容（确认口径 2a）。
///
/// `Ok(pages > 0)` 只代表打印作业已成功提交到 Windows 打印队列（`EndDoc` 成功），
/// 不代表纸张已物理出完；标题与文案因此一律用「已提交 / 已发送」，不写「已完成」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintSuccessView {
    pub title: &'static str,
    /// 打印件名称，来自 `ctp_app::print::doc_name`。
    pub doc_name: String,
    pub sheets: usize,
    pub pages: usize,
    /// 本地时间 `YYYY-MM-DD HH:MM:SS`，仅用于展示。
    pub submitted_at: String,
    /// 弹窗创建时刻，用于 5 秒自动关闭与倒计时。
    pub at: Instant,
}

impl PrintSuccessView {
    pub fn new(doc_name: String, sheets: usize, pages: usize) -> Self {
        Self {
            title: PRINT_SUCCESS_TITLE,
            doc_name,
            sheets,
            pages,
            submitted_at: ctp_core::date::now_local_text(),
            at: Instant::now(),
        }
    }
}

/// 是否已到 5 秒自动关闭时点。
pub fn print_success_expired(v: &PrintSuccessView) -> bool {
    v.at.elapsed() >= Duration::from_secs(PRINT_SUCCESS_SECS)
}

/// 剩余秒数（向上取整、展示时恒 >= 1），用于弹窗里的倒计时文案。
pub fn print_success_remaining_secs(v: &PrintSuccessView) -> u64 {
    let left = Duration::from_secs(PRINT_SUCCESS_SECS).saturating_sub(v.at.elapsed());
    let secs = left.as_millis().div_ceil(1000);
    u64::try_from(secs).unwrap_or(1).max(1)
}

/// 同一时间只允许一个成功提示；已有未关闭的提示时忽略后续成功，避免覆盖 / 重复弹出。
pub fn offer_print_success(slot: &mut Option<PrintSuccessView>, v: PrintSuccessView) -> bool {
    if slot.is_some() {
        return false;
    }
    *slot = Some(v);
    true
}

/// 关闭成功提示：手动「关闭」按钮、5 秒到期、关闭预览窗、登出共用。
pub fn dismiss_print_success(slot: &mut Option<PrintSuccessView>) {
    *slot = None;
}

/// 任一打印提示在场时禁止再次发起打印 / 打开预览（确认口径 4a）。
/// 失败弹窗需手动关闭，成功弹窗 5 秒后自动关闭，两者期间都算「忙」。
pub fn print_busy(failure: &Option<PrintFailureView>, success: &Option<PrintSuccessView>) -> bool {
    failure.is_some() || success.is_some()
}

/// 展示失败提示（确认口径 5a）：先清掉可能还在倒计时的成功提示，
/// 再沿用 R-42 既有的「保留首次原因、不重复弹出」去重逻辑。
pub fn offer_print_failure_exclusive(
    failure: &mut Option<PrintFailureView>,
    success: &mut Option<PrintSuccessView>,
    v: PrintFailureView,
) -> bool {
    dismiss_print_success(success);
    offer_print_failure(failure, v)
}

/// 只有 `Ok(pages > 0)` 才生成成功提示；`Ok(0)`（用户取消）与 `Err` 一律返回 None。
///
/// 与 [`failure_for_outcome`] 对称：打印完成后的三种结果由这两个纯函数分流，
/// 调用方不会漏判，也不必自己记「取消要静默」。
pub fn success_for_outcome(
    outcome: &std::result::Result<usize, ctp_app::print::PrintError>,
    doc_name: String,
    sheets: usize,
) -> Option<PrintSuccessView> {
    match outcome {
        Ok(pages) if *pages > 0 => Some(PrintSuccessView::new(doc_name, sheets, *pages)),
        _ => None,
    }
}

impl ImportResultView {
    pub fn from_report(r: &ctp_app::batch::ImportReport) -> Self {
        ImportResultView {
            imported: r.imported,
            skipped: r.skipped,
            failures: r
                .failures
                .iter()
                .map(|f| (f.line, f.message.clone()))
                .collect(),
            rejected: None,
        }
    }

    pub fn rejected(reason: impl Into<String>) -> Self {
        ImportResultView {
            imported: 0,
            skipped: 0,
            failures: Vec::new(),
            rejected: Some(reason.into()),
        }
    }

    /// 三种场景共用的状态：整份拒绝 / 全部失败为红，部分失败为黄，全成功为绿。
    fn tone(&self) -> Tone {
        if self.rejected.is_some() || (self.imported == 0 && !self.failures.is_empty()) {
            Tone::Danger
        } else if !self.failures.is_empty() {
            Tone::Warn
        } else {
            Tone::Success
        }
    }

    fn headline(&self) -> &'static str {
        if self.rejected.is_some() {
            "整份文件已拒绝"
        } else if self.imported == 0 && !self.failures.is_empty() {
            "全部失败"
        } else if !self.failures.is_empty() {
            "部分成功、部分失败"
        } else {
            "导入完成"
        }
    }
}

/// 新建时暂存于内存的附件。点「保存合同」才随合同一起落库，
/// 避免合同保存失败时在库里留下孤儿附件。
pub struct PendingAttachment {
    pub kind: AttachmentKind,
    pub file_name: String,
    pub mime: Option<String>,
    pub bytes: Vec<u8>,
}

impl PendingAttachment {
    pub fn size_label(&self) -> String {
        let n = self.bytes.len();
        if n < 1024 {
            format!("{n} B")
        } else if n < 1024 * 1024 {
            format!("{:.0} KB", n as f64 / 1024.0)
        } else {
            format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
        }
    }
}

/// 新建合同表单（两步：基本信息 → 附件）。
pub struct ContractForm {
    /// 0 = 基本信息，1 = 附件
    pub step: usize,
    pub name: String,
    /// 所属项目：可输入项目编码或名称
    pub project_text: String,
    /// 承包单位：可输入任意名称
    pub contractor_text: String,
    /// 发包人（甲方）：可输入任意名称
    pub party_a: String,
    pub ctype_idx: usize,
    pub category_idx: usize,
    pub sign_date: String,
    pub amount: String,
    pub duration: String,
    pub planned_start: String,
    pub planned_end: String,
    /// R-50：上一次由「工期 + 计划开工」自动写入的值。
    /// 与当前 `planned_end` 一致 = 仍在自动状态；不一致 = 用户手改过，停止同步。
    pub planned_end_auto: Option<String>,
    /// R-50：用户是否在本次弹窗里动过「计划交工」。
    /// 只影响提示文案——打开弹窗时既有的旧值不该被标成「已手动修改」。
    pub planned_end_dirty: bool,
    /// 交工日期（YYYY-MM-DD），选填。
    pub delivery_date: String,
    pub pricing_idx: usize,
    pub retention: String,
    pub terms: String,
    pub remark: String,
    /// 招标方式下拉索引：0 = 未选，1/2/3 对应公开招标/政府采购/直接发包。
    pub bid_method_idx: usize,
    /// 开标时间（YYYY-MM-DD），选填。
    pub open_bid_date: String,
    /// 项目负责人，选填。
    pub project_leader: String,
    /// 联系方式，选填。
    pub contact_info: String,
    /// 招标代理名称（保存时若为新名称，自动建档为 Agent 角色）。
    pub bid_agent_text: String,
    /// 保存后立即置为「生效」，否则存为草稿。
    pub activate: bool,
    /// 四类附件，每类至多一条
    pub attachments: Vec<PendingAttachment>,
}

impl Default for ContractForm {
    fn default() -> Self {
        Self {
            step: 0,
            name: String::new(),
            project_text: String::new(),
            contractor_text: String::new(),
            party_a: String::new(),
            ctype_idx: 0,
            category_idx: 0,
            sign_date: today_iso(),
            amount: String::new(),
            duration: String::new(),
            planned_start: String::new(),
            planned_end: String::new(),
            planned_end_auto: None,
            planned_end_dirty: false,
            delivery_date: String::new(),
            pricing_idx: 1,
            retention: "3".into(),
            terms: "按月计量支付，进度款支付至已完工程量的 80%，竣工验收后支付至 97%，质保金 3% 满一年退还。".into(),
            remark: String::new(),
            bid_method_idx: 1,
            open_bid_date: String::new(),
            project_leader: String::new(),
            contact_info: String::new(),
            bid_agent_text: String::new(),
            activate: true,
            attachments: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------- 合同信息修改（R-50）

/// 合同主档修改表单。
///
/// 字段范围与 `service::EditContract` 一致：**金额、编号、审计价、状态、交工日期
/// 都不在这里**，它们各有专门的留痕入口，从这里改会绕过历史表。
pub struct EditContractForm {
    pub contract_id: String,
    /// 只读展示，不可修改
    pub contract_no: String,
    pub name: String,
    /// 项目名称（不含编码），与新建时手输的内容一致；编码只在联想候选里作为检索线索
    pub project_text: String,
    pub contractor_text: String,
    pub party_a: String,
    pub ctype_idx: usize,
    pub category_idx: usize,
    pub sign_date: String,
    pub duration: String,
    pub planned_start: String,
    pub planned_end: String,
    pub planned_end_auto: Option<String>,
    pub planned_end_dirty: bool,
    pub pricing_idx: usize,
    pub retention: String,
    pub terms: String,
    pub remark: String,
    pub bid_method_idx: usize,
    pub open_bid_date: String,
    pub project_leader: String,
    pub contact_info: String,
    pub bid_agent_text: String,
    /// 必填：任何主档变更都要能回答「为什么改」
    pub reason: String,
    pub error: Option<String>,
}

impl EditContractForm {
    /// 用当前合同预填表单。
    ///
    /// 项目 / 承包单位 / 招标代理要转成展示名，否则用户打开看到的是内部 id。
    /// 项目只填**名称**、不带编码：新建时用户录的就是名称，两处显示保持一致，
    /// 也避免编码前缀留在框里被误当成「已选定的项目」（R-51）。
    /// `planned_end_auto` 留空：进入修改页时既有交工日期一律视为「已有值」，
    /// 只要用户不改工期与开工日，就不会被自动计算覆盖。
    pub fn from_contract(
        c: &Contract,
        projects: &[ctp_core::project::Project],
        contractors: &[ctp_core::project::Contractor],
    ) -> Self {
        let project_text = projects
            .iter()
            .find(|p| p.id == c.project_id)
            .map(|p| p.project_name.clone())
            .unwrap_or_default();
        let contractor_text = contractors
            .iter()
            .find(|x| x.id == c.contractor_id)
            .map(|x| x.contractor_name.clone())
            .unwrap_or_default();
        let bid_agent_text = c
            .bid_agent_id
            .as_ref()
            .and_then(|id| contractors.iter().find(|x| &x.id == id))
            .map(|x| x.contractor_name.clone())
            .unwrap_or_default();

        let ctype_idx = CONTRACT_TYPES
            .iter()
            .position(|t| *t == c.contract_type)
            .unwrap_or(0);
        let category_idx = dict::CONTRACT_CATEGORIES
            .iter()
            .position(|(code, _)| *code == c.contract_category_code)
            .unwrap_or(0);
        let pricing_idx = match c.pricing_method {
            None => 0,
            Some(PricingMethod::LumpSum) => 1,
            Some(PricingMethod::UnitPrice) => 2,
            Some(PricingMethod::CostPlus) => 3,
        };
        let bid_method_idx = match c.bid_method {
            None => 0,
            Some(BidMethod::Open) => 1,
            Some(BidMethod::Government) => 2,
            Some(BidMethod::Direct) => 3,
        };

        Self {
            contract_id: c.id.clone(),
            contract_no: c.contract_no.clone(),
            name: c.contract_name.clone(),
            project_text,
            contractor_text,
            party_a: c.party_a.clone(),
            ctype_idx,
            category_idx,
            sign_date: c.sign_date.clone(),
            duration: if c.duration_days > 0 {
                c.duration_days.to_string()
            } else {
                String::new()
            },
            planned_start: c.planned_start.clone().unwrap_or_default(),
            planned_end: c.planned_end.clone().unwrap_or_default(),
            planned_end_auto: None,
            planned_end_dirty: false,
            pricing_idx,
            retention: bp_text(c.retention_rate_bp),
            terms: c.payment_terms.clone(),
            remark: c.remark.clone().unwrap_or_default(),
            bid_method_idx,
            open_bid_date: c.open_bid_date.clone().unwrap_or_default(),
            project_leader: c.project_leader.clone().unwrap_or_default(),
            contact_info: c.contact_info.clone().unwrap_or_default(),
            bid_agent_text,
            reason: String::new(),
            error: None,
        }
    }
}

/// 万分比 → 百分比文本（300 → "3"，350 → "3.5"）。
fn bp_text(bp: i32) -> String {
    let v = f64::from(bp) / 100.0;
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

impl ContractForm {
    pub fn attachment(&self, kind: AttachmentKind) -> Option<&PendingAttachment> {
        self.attachments.iter().find(|a| a.kind == kind)
    }

    pub fn has_contract_copy(&self) -> bool {
        self.attachment(AttachmentKind::Contract).is_some()
    }
}

/// 拨款登记表单。
pub struct PayForm {
    pub amount: String,
    pub date: String,
    pub reason: String,
    pub method_idx: usize,
}

impl Default for PayForm {
    fn default() -> Self {
        Self {
            amount: String::new(),
            date: today_iso(),
            reason: String::new(),
            method_idx: 0,
        }
    }
}

/// 审计价表单。
pub struct AuditForm {
    pub amount: String,
    pub date: String,
    pub org: String,
    pub report_no: String,
    pub reason: String,
}

impl Default for AuditForm {
    fn default() -> Self {
        Self {
            amount: String::new(),
            date: today_iso(),
            org: String::new(),
            report_no: String::new(),
            reason: "结算审定".into(),
        }
    }
}

/// 合同金额编辑表单。
#[derive(Default)]
pub struct AmountForm {
    pub amount: String,
    pub reason: String,
}

const PAY_METHODS: [PayMethod; 4] = [
    PayMethod::Transfer,
    PayMethod::Acceptance,
    PayMethod::Cash,
    PayMethod::Other,
];

const CONTRACT_TYPES: [ContractType; 6] = [
    ContractType::Construct,
    ContractType::Supervise,
    ContractType::Design,
    ContractType::Survey,
    ContractType::Supply,
    ContractType::Service,
];

const PRICING: [&str; 4] = ["未填写", "总价", "单价", "成本加酬金"];

fn modal(title: impl Into<String>, p: &Palette) -> egui::Window<'static> {
    egui::Window::new(title.into())
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .frame(
            egui::Frame::none()
                .fill(p.surface)
                .stroke(egui::Stroke::new(1.0_f32, p.border))
                .rounding(egui::Rounding::same(14.0))
                .inner_margin(egui::Margin::same(20.0)),
        )
}

fn field(ui: &mut egui::Ui, p: &Palette, label: &str, w: f32, value: &mut String) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        widgets::field_label(ui, p, label);
        ui.add_sized(
            [w, 26.0],
            egui::TextEdit::singleline(value).margin(egui::Margin::symmetric(8.0, 4.0)),
        );
    });
}

/// 带标签的日期输入框：点击输入框弹出月历，选中回填。
fn date_field(ui: &mut egui::Ui, p: &Palette, label: &str, id: &str, w: f32, value: &mut String) {
    let _ = date_field_note(ui, p, label, id, w, value, None);
}

/// 日期输入框 + 字段下方的灰色小字提示（R-50 用来标「已手动修改」）。
///
/// 返回 `true` 表示本帧用户改了这个字段（用于区分「用户手改」与「打开时就有的旧值」）。
fn date_field_note(
    ui: &mut egui::Ui,
    p: &Palette,
    label: &str,
    id: &str,
    w: f32,
    value: &mut String,
    note: Option<&str>,
) -> bool {
    let before = value.clone();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        widgets::field_label(ui, p, label);
        widgets::date_input(ui, id, p, value, w);
        if let Some(n) = note {
            ui.label(RichText::new(n).size(10.5).color(p.text_muted));
        }
    });
    *value != before
}

// ---------------------------------------------------------------- 计划交工自动计算（R-50）

/// 按「计划开工 + 工期 = 计划交工」自动回填（**含开工当日**）。
///
/// 触发条件：工期是正整数、计划开工是合法日期，两者**都**成立才计算；
/// 任一不成立就原样返回 `false`，绝不改动用户已填的内容。
///
/// 写入规则（对应「手改后停止同步」口径）：
/// * `end` 为空 → 视为未填，直接写入；
/// * `end` 仍等于上次自动写入的值（`last_auto`）→ 视为仍在自动状态，更新为新结果；
/// * `end` 既非空又不同于 `last_auto` → 用户手改过，**不再覆盖**。
///
/// 返回 `true` 表示本帧确实写入了新值。
pub fn auto_planned_end(
    planned_start: &str,
    duration: &str,
    end: &mut String,
    last_auto: &mut Option<String>,
) -> bool {
    let days = duration.trim().parse::<i64>().unwrap_or(0);
    let Some(want) = ctp_core::contract::planned_end_for(planned_start.trim(), days) else {
        return false;
    };
    let still_auto = last_auto.as_deref() == Some(end.as_str());
    if !(end.trim().is_empty() || still_auto) {
        return false;
    }
    if end.as_str() == want.as_str() {
        return false;
    }
    *end = want.clone();
    *last_auto = Some(want);
    true
}

/// 计划交工是否已被人工修改（用于字段下方提示）。
///
/// 空字段算「未填」，不算手改；一旦填了且与自动值不同，就是手改。
pub fn planned_end_is_manual(end: &str, last_auto: &Option<String>) -> bool {
    !end.trim().is_empty() && last_auto.as_deref() != Some(end)
}

/// 下拉框（索引式）。
fn combo(
    ui: &mut egui::Ui,
    id: &str,
    width: f32,
    current: usize,
    labels: Vec<String>,
    sel: &mut usize,
) {
    let text = labels
        .get(current)
        .cloned()
        .unwrap_or_else(|| "（无）".to_string());
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(text)
        .show_ui(ui, |ui| {
            for (i, l) in labels.iter().enumerate() {
                ui.selectable_value(sel, i, l);
            }
        });
}

/// R-46：合同台账导出前的列选择。
///
/// 只做「勾选 + 调顺序」，确认后把快照结果投影成只含所选列的 `ReportResult`，
/// 再交给既有的 `export_report_result` 走原来的保存框 / 审计 / 提示链路。
fn export_columns(app: &mut App, ctx: &egui::Context) {
    let Some(mut view) = app.export_columns.clone() else {
        dismiss_export_columns(&mut app.dialog, &mut app.export_columns);
        return;
    };
    let p = app.pal;
    let mut cancel = false;
    let mut submit = false;
    let mut all_on = false;
    let mut all_off = false;
    let mut reset = false;
    let mut move_up: Option<usize> = None;
    let mut move_down: Option<usize> = None;
    let n = view.choices.len();
    let selected_count = view.choices.iter().filter(|c| c.selected).count();
    let row_count = view.result.rows.len();
    let error_text = view.error.clone();

    egui::Window::new("导出合同台账 · 选择列")
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .frame(
            egui::Frame::none()
                .fill(p.surface)
                .stroke(egui::Stroke::new(1.0_f32, p.border))
                .rounding(egui::Rounding::same(14.0))
                .inner_margin(egui::Margin::same(20.0)),
        )
        .show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(
                RichText::new("选择要导出的字段，用「上移 / 下移」调整 Excel 中的列顺序。")
                    .size(12.0)
                    .color(p.text_muted),
            );
            ui.label(
                RichText::new(format!(
                    "按当前列表筛选与排序导出，共 {row_count} 份合同；已选 {selected_count} / {n} 列"
                ))
                .size(11.5)
                .color(p.text_muted),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::link(ui, &p, "全选", 11.5).clicked() {
                    all_on = true;
                }
                if widgets::link(ui, &p, "全不选", 11.5).clicked() {
                    all_off = true;
                }
                if widgets::link(ui, &p, "恢复默认", 11.5).clicked() {
                    reset = true;
                }
            });
            ui.add_space(6.0);
            widgets::divider(ui, &p);
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("export_columns_list")
                .max_height(320.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, c) in view.choices.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut c.selected, "");
                            ui.add_sized(
                                [250.0, 24.0],
                                egui::Label::new(
                                    RichText::new(c.label.as_str()).size(12.5).color(p.text),
                                ),
                            );
                            if widgets::ghost_button_sm_on(ui, &p, "上移", i > 0).clicked()
                                && i > 0
                            {
                                move_up = Some(i);
                            }
                            if widgets::ghost_button_sm_on(ui, &p, "下移", i + 1 < n).clicked()
                                && i + 1 < n
                            {
                                move_down = Some(i);
                            }
                        });
                    }
                });
            if let Some(err) = &error_text {
                ui.add_space(6.0);
                ui.label(RichText::new(err).size(11.5).color(p.danger));
            }
            ui.add_space(10.0);
            widgets::divider(ui, &p);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &p, "确认导出", selected_count > 0).clicked()
                    && selected_count > 0
                {
                    submit = true;
                }
                if widgets::ghost_button(ui, &p, "取消").clicked() {
                    cancel = true;
                }
            });
        });

    if all_on {
        view.set_all(true);
    }
    if all_off {
        view.set_all(false);
    }
    if reset {
        view.reset();
    }
    if let Some(i) = move_up {
        view.move_up(i);
    }
    if let Some(i) = move_down {
        view.move_down(i);
    }
    app.export_columns = Some(view.clone());

    if cancel {
        dismiss_export_columns(&mut app.dialog, &mut app.export_columns);
        return;
    }
    if submit {
        match view.confirm() {
            Ok(result) => {
                dismiss_export_columns(&mut app.dialog, &mut app.export_columns);
                app.export_report_result(&result);
            }
            // 空选择由置灰按钮挡住；其余校验错误沿用原来的「导出失败」提示渠道。
            Err(e) => app.error(format!("导出失败：{e}")),
        }
    }
}

pub fn draw(app: &mut App, ctx: &egui::Context) {
    // 弹窗打开时压暗底图，把注意力收到弹窗上。
    // 该图层在弹窗图层之前创建，因此始终绘制在弹窗下方；它不参与交互，点击可正常穿透到按钮。
    if app.dialog != Dialog::None
        || app.confirm.is_some()
        || app.print_failure.is_some()
        || app.print_success.is_some()
        || app.edit_done.is_some()
    {
        let alpha = if app.mode.is_dark() { 130 } else { 55 };
        // 画在背景层末尾：盖住主界面，但仍在 Order::Middle 的窗口之下
        ctx.layer_painter(egui::LayerId::background()).rect_filled(
            ctx.screen_rect(),
            egui::Rounding::ZERO,
            egui::Color32::from_black_alpha(alpha),
        );
    }
    // R-33：导入结果弹窗是真模态。全屏交互层吃掉所有点击，背后界面仍可见但不可操作；
    // Esc、点弹窗外、刷新、切换主题都不会关闭它，只有弹窗内的「关闭」按钮能退出。
    //
    // 层级必须是 Middle：egui 0.29 的 Order 顺序为
    // Background → Panel → Middle → Foreground → Tooltip → Debug，Tooltip 高于
    // Foreground，若拦截层用 Tooltip 会盖住弹窗，导致「关闭」收不到点击。
    // 拦截层在 Middle（低于弹窗的 Foreground，高于背景主界面）才能既挡背景又可关窗。
    if matches!(app.dialog, Dialog::ImportResult | Dialog::ExportColumns) {
        let rect = ctx.screen_rect();
        egui::Area::new(egui::Id::new("modal_dialog_blocker"))
            .order(egui::Order::Middle)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                let _ = ui.allocate_response(rect.size(), egui::Sense::click());
            });
    }
    match app.dialog {
        Dialog::None => {}
        Dialog::NewContract => new_contract(app, ctx),
        Dialog::AuditPrice => audit_price(app, ctx),
        Dialog::DeliveryDate => delivery_date(app, ctx),
        Dialog::EditAmount => edit_amount(app, ctx),
        Dialog::PaymentDetail => payment_detail(app, ctx),
        Dialog::PrintPreview => print_preview(app, ctx),
        Dialog::Attachments => attachments(app, ctx),
        Dialog::MaterialPrint => material_print(app, ctx),
        Dialog::ContractDetail => contract_detail(app, ctx),
        Dialog::ImportResult => import_result(app, ctx),
        Dialog::ExportColumns => export_columns(app, ctx),
        Dialog::EditContract => edit_contract(app, ctx),
    }
    if app.confirm.is_some() {
        confirm_void(app, ctx);
    }
    // R-42：打印失败弹窗是叠加层，不改变 `app.dialog`。预览窗仍在下面，
    // 关掉提示即可回到原来的单据与页码；用 Foreground 保证它盖在预览窗之上。
    // R-44：成功提示是并列的另一个叠加层，失败优先，二者不会同时绘制。
    if app.print_failure.is_some() {
        print_failure_dialog(app, ctx);
    } else if app.print_success.is_some() {
        print_success_dialog(app, ctx);
    }
    // R-51：主档修改成功后的「本次修改」弹窗，同样是与 Dialog 解耦的叠加层
    if app.edit_done.is_some() {
        edit_done_dialog(app, ctx);
    }
}

/// 打印失败：所有打印失败的唯一展示入口。
///
/// 标题 / 失败原因 / 建议操作 / 技术信息 / 关闭方式齐备；只认窗口内的「关闭」按钮，
/// Esc 与点击弹窗外部都不会关闭，也不额外再发一条红色提示条。
fn print_failure_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(view) = app.print_failure.clone() else {
        return;
    };
    let p = app.pal;
    let mut close = false;
    egui::Window::new(view.title)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .frame(
            egui::Frame::none()
                .fill(p.surface)
                .stroke(egui::Stroke::new(1.0_f32, p.border))
                .rounding(egui::Rounding::same(14.0))
                .inner_margin(egui::Margin::same(20.0)),
        )
        .show(ctx, |ui| {
            ui.set_width(500.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(view.title)
                        .size(17.0)
                        .strong()
                        .color(p.danger),
                );
                widgets::badge(ui, &p, view.kind, Tone::Danger);
            });
            ui.add_space(10.0);
            widgets::divider(ui, &p);
            ui.add_space(8.0);

            ui.label(RichText::new("失败原因").size(12.5).strong().color(p.text));
            ui.add_space(2.0);
            ui.label(RichText::new(&view.reason).size(13.0).color(p.text));
            ui.add_space(10.0);

            widgets::soft_box(&p, p.primary_soft).show(ui, |ui| {
                ui.set_width(480.0);
                ui.label(
                    RichText::new("建议操作")
                        .size(12.5)
                        .strong()
                        .color(p.primary),
                );
                ui.add_space(2.0);
                ui.label(RichText::new(&view.solution).size(12.5).color(p.text));
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("技术信息：{}", view.technical))
                    .size(11.0)
                    .color(p.text_muted),
            );

            ui.add_space(10.0);
            widgets::divider(ui, &p);
            ui.add_space(6.0);
            ui.label(
                RichText::new("关闭方式：点击下方「关闭」返回；按 Esc 或点击弹窗外部不会关闭。")
                    .size(11.0)
                    .color(p.text_muted),
            );
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::primary_button(ui, &p, "关闭", true).clicked() {
                    close = true;
                }
            });
        });

    if close {
        dismiss_print_failure(&mut app.print_failure);
    }
}

/// 打印成功：作业成功提交后展示本次打印信息，5 秒后自动关闭（R-44）。
///
/// 与失败弹窗同款卡片外观，同样不占用 `Dialog`，关掉后预览单据 / 页码 / 位图全部保留。
/// 提供「关闭」可提前关，并显示剩余秒数；失败弹窗出现时会先清掉本提示，二者不叠加。
fn print_success_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(view) = app.print_success.clone() else {
        return;
    };
    let p = app.pal;
    let mut close = false;
    let left = print_success_remaining_secs(&view);
    egui::Window::new(view.title)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .frame(
            egui::Frame::none()
                .fill(p.surface)
                .stroke(egui::Stroke::new(1.0_f32, p.border))
                .rounding(egui::Rounding::same(14.0))
                .inner_margin(egui::Margin::same(20.0)),
        )
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(view.title)
                        .size(17.0)
                        .strong()
                        .color(p.success),
                );
                widgets::badge(ui, &p, "已发送至打印机", Tone::Success);
            });
            ui.add_space(10.0);
            widgets::divider(ui, &p);
            ui.add_space(8.0);

            ui.label(
                RichText::new("本次打印信息")
                    .size(12.5)
                    .strong()
                    .color(p.text),
            );
            ui.add_space(4.0);
            let rows = [
                ("打印件", view.doc_name.clone()),
                ("单据张数", format!("{} 张", view.sheets)),
                ("输出页数", format!("{} 页", view.pages)),
                ("提交时间", view.submitted_at.clone()),
            ];
            for (k, v) in rows {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(k).size(12.5).color(p.text_muted));
                    ui.add_space(8.0);
                    ui.label(RichText::new(v).size(12.5).color(p.text));
                });
            }
            ui.add_space(10.0);

            widgets::soft_box(&p, p.success_soft).show(ui, |ui| {
                ui.set_width(440.0);
                ui.label(
                    RichText::new(
                        "作业已发送至打印队列，请到打印机取件并核对纸张为 A4 横向。\
                         若队列卡住或缺纸，请以打印机实际出纸为准，不要重复点击「打印」。",
                    )
                    .size(12.0)
                    .color(p.text),
                );
            });

            ui.add_space(10.0);
            widgets::divider(ui, &p);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("本提示将在 {left} 秒后自动关闭"))
                        .size(11.0)
                        .color(p.text_muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::ghost_button(ui, &p, "关闭").clicked() {
                        close = true;
                    }
                });
            });
        });

    if close {
        dismiss_print_success(&mut app.print_success);
    }
}

/// 批量拨款导入结果：常驻真模态，只列统计摘要 + 失败条目，不列逐行明细。
fn import_result(app: &mut App, ctx: &egui::Context) {
    let Some(view) = app.import_result.clone() else {
        dismiss_import_result(&mut app.dialog, &mut app.import_result);
        return;
    };
    let p = app.pal;
    let mut close = false;
    egui::Window::new("批量拨款导入结果")
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .frame(
            egui::Frame::none()
                .fill(p.surface)
                .stroke(egui::Stroke::new(1.0_f32, p.border))
                .rounding(egui::Rounding::same(14.0))
                .inner_margin(egui::Margin::same(20.0)),
        )
        .show(ctx, |ui| {
            ui.set_width(520.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("批量拨款导入结果")
                        .size(17.0)
                        .strong()
                        .color(p.text),
                );
                widgets::badge(ui, &p, view.headline(), view.tone());
            });
            ui.add_space(10.0);

            ui.label(
                RichText::new(format!(
                    "成功导入 {} 笔 · 跳过 {} 行 · 失败 {} 行",
                    view.imported,
                    view.skipped,
                    view.failures.len()
                ))
                .size(13.5)
                .strong()
                .color(p.text),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "本次金额留空或为 0 的行跳过；日期留空按当天、事由留空按「拨付进度款」、支付方式留空按转账。",
                )
                .size(11.5)
                .color(p.text_muted),
            );
            ui.add_space(10.0);

            if let Some(reason) = &view.rejected {
                widgets::divider(ui, &p);
                ui.add_space(6.0);
                ui.label(RichText::new("拒绝原因").size(12.5).strong().color(p.danger));
                ui.label(RichText::new(reason).size(12.0).color(p.text));
                ui.add_space(8.0);
            }

            if !view.failures.is_empty() {
                widgets::divider(ui, &p);
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("失败明细（{} 行）", view.failures.len()))
                        .size(12.5)
                        .strong()
                        .color(p.danger),
                );
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt("import_failures_scroll")
                    .max_height(220.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for (line, msg) in &view.failures {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(format!("第 {line} 行"))
                                        .size(12.0)
                                        .strong()
                                        .color(p.danger),
                                );
                                ui.label(RichText::new(msg).size(12.0).color(p.text));
                            });
                        }
                    });
                ui.add_space(8.0);
            }

            widgets::divider(ui, &p);
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::primary_button(ui, &p, "关闭", true).clicked() {
                    close = true;
                }
            });
        });

    if close {
        dismiss_import_result(&mut app.dialog, &mut app.import_result);
    }
}

// ---------------------------------------------------------------- 新建合同

/// MIME 粗映射：只用于给附件加个类型标注，判断不出来不影响存储。
fn mime_of(file_name: &str) -> Option<&'static str> {
    let ext = file_name.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "txt" => "text/plain",
        _ => return None,
    })
}

/// 弹系统文件对话框，把文件读进内存；超过 50 MB 直接拒绝。
fn pick_attachment(kind: AttachmentKind) -> Result<PendingAttachment, String> {
    let path = rfd::FileDialog::new()
        .set_title(format!("选择{}", kind.label()))
        .add_filter(
            "常见附件",
            &[
                "pdf", "doc", "docx", "xls", "xlsx", "jpg", "jpeg", "png", "txt",
            ],
        )
        .add_filter("全部文件", &["*"])
        .pick_file()
        .ok_or_else(|| "已取消选择".to_string())?;

    let meta = std::fs::metadata(&path).map_err(|e| format!("读取文件信息失败：{e}"))?;
    let max = ctp_core::attachment::MAX_ATTACHMENT_BYTES;
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "附件".to_string());
    if meta.len() as i64 > max {
        return Err(format!(
            "「{file_name}」{:.1} MB，超过 {} MB 上限",
            meta.len() as f64 / (1024.0 * 1024.0),
            max / 1024 / 1024
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("读取文件失败：{e}"))?;
    if bytes.is_empty() {
        return Err(format!("「{file_name}」内容为空"));
    }
    Ok(PendingAttachment {
        kind,
        mime: mime_of(&file_name).map(|s| s.to_string()),
        file_name,
        bytes,
    })
}

/// 步骤胶囊：「1 基本信息 → 2 附件」。
fn step_chip(ui: &mut egui::Ui, p: &Palette, no: u32, label: &str, active: bool) {
    let (fg, bg) = if active {
        Tone::Primary.colors(p)
    } else {
        Tone::Neutral.colors(p)
    };
    egui::Frame::none()
        .fill(bg)
        .rounding(egui::Rounding::same(20.0))
        .inner_margin(egui::Margin::symmetric(11.0, 4.0))
        .show(ui, |ui| {
            ui.label(RichText::new(format!("{no} {label}")).size(12.0).color(fg));
        });
}

/// 带字段标签的联想输入框。
// 表单字段渲染需要一次性拿到标签、联想源、模式等上下文，保持扁平签名。
#[allow(clippy::too_many_arguments)]
fn ac_field(
    ui: &mut egui::Ui,
    p: &Palette,
    label: &str,
    id: &str,
    text: &mut String,
    hints: &[String],
    hint_text: &str,
    w: f32,
    mode: widgets::SuggestMode,
) -> Option<String> {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        widgets::field_label(ui, p, label);
        widgets::autocomplete(ui, id, p, text, hints, w, hint_text, mode)
    })
    .inner
}

/// 带字段标签的联想输入框：点选候选时写入 `pick` 换算出的值。
// 与 `ac_field` 同一组入参，只是多一个回填换算。
#[allow(clippy::too_many_arguments)]
fn ac_field_pick(
    ui: &mut egui::Ui,
    p: &Palette,
    label: &str,
    id: &str,
    text: &mut String,
    hints: &[String],
    hint_text: &str,
    w: f32,
    mode: widgets::SuggestMode,
    pick: &dyn Fn(&str) -> String,
) -> Option<String> {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        widgets::field_label(ui, p, label);
        widgets::autocomplete_pick(ui, id, p, text, hints, w, hint_text, mode, pick)
    })
    .inner
}

/// 项目候选对：（列表里显示的「编码 名称」，回填用的「名称」）。
///
/// 列表显示带编码是为了能用编码检索；回填只写名称，保证修改窗里看到的内容
/// 与新建时手输的一致（R-51）。
fn project_sources(projects: &[ctp_core::project::Project]) -> Vec<(String, String)> {
    projects
        .iter()
        .map(|x| {
            (
                format!("{} {}", x.project_code, x.project_name),
                x.project_name.clone(),
            )
        })
        .collect()
}

/// 点选项目候选后回填的名称；候选之外的原文原样保留（允许录新项目）。
fn pick_project_name(sources: &[(String, String)], picked: &str) -> String {
    sources
        .iter()
        .find(|(label, _)| label == picked)
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| picked.to_string())
}

/// 新建合同专用紧凑卡片：内边距 12（全局卡片为 16），为「一屏不滚动」省高度。
fn form_card(p: &Palette) -> egui::Frame {
    egui::Frame::none()
        .fill(p.surface)
        .stroke(egui::Stroke::new(1.0_f32, p.border))
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::same(12.0))
}

fn new_contract(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;

    // 候选先克隆出来：闭包里要可变借用 `app.new_contract`，不能再读 `app.projects`
    let project_src = project_sources(&app.projects);
    let project_hints: Vec<String> = project_src.iter().map(|(l, _)| l.clone()).collect();
    let mut unit_hints: Vec<String> = app
        .contractors
        .iter()
        .filter(|c| c.role_type != ContractorRole::Owner)
        .map(|c| c.contractor_name.clone())
        .collect();
    unit_hints.sort();
    unit_hints.dedup();
    let mut owner_hints: Vec<String> = app
        .contractors
        .iter()
        .filter(|c| c.role_type == ContractorRole::Owner)
        .map(|c| c.contractor_name.clone())
        .collect();
    owner_hints.extend(app.party_a_names.iter().cloned());
    owner_hints.sort();
    owner_hints.dedup();

    let mut agent_hints: Vec<String> = app
        .contractors
        .iter()
        .filter(|c| c.role_type == ContractorRole::Agent)
        .map(|c| c.contractor_name.clone())
        .collect();
    // 库里还没有「招标代理」档案时退化为全部单位，避免联想框看起来是空的
    if agent_hints.is_empty() {
        agent_hints = app
            .contractors
            .iter()
            .map(|c| c.contractor_name.clone())
            .collect();
    }
    agent_hints.sort();
    agent_hints.dedup();

    // 第一步的完整性：与保存时的校验共用同一套条件
    let amount_ok = Money::parse_yuan(&app.new_contract.amount)
        .map(|m| m.cent() > 0)
        .unwrap_or(false);
    let base_ok = !app.new_contract.name.trim().is_empty()
        && !app.new_contract.project_text.trim().is_empty()
        && !app.new_contract.contractor_text.trim().is_empty()
        && !app.new_contract.party_a.trim().is_empty()
        && amount_ok
        && is_iso_date(&app.new_contract.sign_date)
        && !app.new_contract.terms.trim().is_empty();

    let step = app.new_contract.step;
    let activate = app.new_contract.activate;
    let has_contract_copy = app.new_contract.has_contract_copy();
    // 勾了「立即生效」就必须先有合同附件，否则生效那一步必然被业务规则拒掉
    let can_save = base_ok && (!activate || has_contract_copy);

    let mut submit = false;
    let mut cancel = false;
    let mut next = false;
    let mut back = false;
    let mut pick: Option<AttachmentKind> = None;
    let mut remove: Option<AttachmentKind> = None;
    let mut flash: Option<String> = None;

    modal("新建合同", &p)
        .default_width(1060.0)
        .min_width(1060.0)
        // 长表单用居中锚点在首帧尺寸未定时会顶出窗口，改为顶部固定留白
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 16.0))
        .show(ctx, |ui| {
            ui.set_width(1020.0);
            ui.horizontal(|ui| {
                step_chip(ui, &p, 1, "基本信息", step == 0);
                ui.label(RichText::new("→").size(12.0).color(p.text_muted));
                step_chip(ui, &p, 2, "附件", step == 1);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("合同编号按「HT-年份-流水」自动生成；带 * 为必填")
                            .size(11.5)
                            .color(p.text_muted),
                    );
                });
            });
            ui.add_space(12.0);

            if step == 0 {
                // 两列卡片：左列（主体 / 招标与工期 / 负责人），右列（价款与条款）。
                // 整页不滚动，靠两列压缩高度，保证一屏内完整展示。
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(486.0);
                        form_card(&p).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            widgets::section_title(ui, &p, "合同主体", "含负责人与联系，可输新名称自动建档");
                            egui::Grid::new("card_subject")
                                .num_columns(2)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    field(ui, &p, "* 合同名称", 218.0, &mut app.new_contract.name);
                                    ac_field(ui, &p, "* 发包人（甲方）", "party_a", &mut app.new_contract.party_a, &owner_hints, "输入名称，可从已有业主中联想", 218.0, widgets::SuggestMode::ShowAllOnEmpty);
                                    ui.end_row();
                                    ac_field_pick(ui, &p, "* 所属项目", "project", &mut app.new_contract.project_text, &project_hints, "输入编码或名称后联想", 218.0, widgets::SuggestMode::TypedOnly, &|s| pick_project_name(&project_src, s));
                                    ac_field(ui, &p, "* 承包单位", "contractor", &mut app.new_contract.contractor_text, &unit_hints, "输入名称后联想已有单位", 218.0, widgets::SuggestMode::TypedOnly);
                                    ui.end_row();
                                    field(ui, &p, "项目负责人", 218.0, &mut app.new_contract.project_leader);
                                    field(ui, &p, "联系方式", 218.0, &mut app.new_contract.contact_info);
                                    ui.end_row();
                                });
                        });
                        ui.add_space(10.0);

                        form_card(&p).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            widgets::section_title(ui, &p, "招标与工期", "");
                            egui::Grid::new("card_bid")
                                .num_columns(2)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    ui.vertical(|ui| {
                                        widgets::field_label(ui, &p, "招标方式");
                                        let bid_labels: Vec<String> = vec![
                                            "（未选）".into(),
                                            "公开招标".into(),
                                            "政府采购".into(),
                                            "直接发包".into(),
                                        ];
                                        combo(ui, "bid_method", 218.0, app.new_contract.bid_method_idx, bid_labels, &mut app.new_contract.bid_method_idx);
                                    });
                                    date_field(ui, &p, "开标时间", "open_bid_date", 218.0, &mut app.new_contract.open_bid_date);
                                    ui.end_row();
                                    ac_field(ui, &p, "招标代理", "bid_agent", &mut app.new_contract.bid_agent_text, &agent_hints, "输入名称后联想；新名称自动建档", 218.0, widgets::SuggestMode::TypedOnly);
                                    field(ui, &p, "工期（日历天）", 218.0, &mut app.new_contract.duration);
                                    ui.end_row();
                                    date_field(ui, &p, "计划开工", "planned_start", 218.0, &mut app.new_contract.planned_start);
                                    // R-50：工期与开工日都有效时，自动算「计划交工」（含开工当日）。
                                    // 用户手改过就不再覆盖，只在字段下方提示。
                                    auto_planned_end(
                                        &app.new_contract.planned_start,
                                        &app.new_contract.duration,
                                        &mut app.new_contract.planned_end,
                                        &mut app.new_contract.planned_end_auto,
                                    );
                                    let end_manual = planned_end_is_manual(
                                        &app.new_contract.planned_end,
                                        &app.new_contract.planned_end_auto,
                                    );
                                    let show_note =
                                        app.new_contract.planned_end_dirty && end_manual;
                                    let end_changed = date_field_note(
                                        ui,
                                        &p,
                                        "计划交工",
                                        "planned_end",
                                        218.0,
                                        &mut app.new_contract.planned_end,
                                        if show_note {
                                            Some("已手动修改，不再随工期自动同步")
                                        } else {
                                            None
                                        },
                                    );
                                    if end_changed {
                                        app.new_contract.planned_end_dirty = true;
                                    }
                                    ui.end_row();
                                });
                        });
                    });

                    ui.add_space(12.0);

                    ui.vertical(|ui| {
                        ui.set_width(486.0);
                        form_card(&p).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            widgets::section_title(ui, &p, "价款与条款", "");
                            egui::Grid::new("card_price")
                                .num_columns(2)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    ui.vertical(|ui| {
                                        widgets::field_label(ui, &p, "合同类型");
                                        let labels: Vec<String> = CONTRACT_TYPES.iter().map(|t| t.label().to_string()).collect();
                                        combo(ui, "ctype", 218.0, app.new_contract.ctype_idx, labels, &mut app.new_contract.ctype_idx);
                                    });
                                    ui.vertical(|ui| {
                                        widgets::field_label(ui, &p, "* 合同类别");
                                        let labels: Vec<String> = dict::CONTRACT_CATEGORIES.iter().map(|(_, n)| n.to_string()).collect();
                                        combo(ui, "category", 218.0, app.new_contract.category_idx, labels, &mut app.new_contract.category_idx);
                                    });
                                    ui.end_row();
                                    date_field(ui, &p, "* 签订日期", "sign_date", 218.0, &mut app.new_contract.sign_date);
                                    field(ui, &p, "* 合同金额（元）", 218.0, &mut app.new_contract.amount);
                                    ui.end_row();
                                    ui.vertical(|ui| {
                                        widgets::field_label(ui, &p, "计价方式");
                                        let labels: Vec<String> = PRICING.iter().map(|x| x.to_string()).collect();
                                        combo(ui, "pricing", 218.0, app.new_contract.pricing_idx, labels, &mut app.new_contract.pricing_idx);
                                    });
                                    field(ui, &p, "质保金率 %", 218.0, &mut app.new_contract.retention);
                                    ui.end_row();
                                    date_field(ui, &p, "交工日期", "delivery_date", 218.0, &mut app.new_contract.delivery_date);
                                    ui.label("");
                                    ui.end_row();
                                });
                            ui.add_space(6.0);
                            widgets::field_label(ui, &p, "* 付款条款");
                            ui.add(egui::TextEdit::multiline(&mut app.new_contract.terms).desired_rows(3).desired_width(ui.available_width()).margin(egui::Margin::symmetric(8.0, 6.0)));
                            ui.add_space(6.0);
                            widgets::field_label(ui, &p, "备注");
                            ui.add(egui::TextEdit::multiline(&mut app.new_contract.remark).desired_rows(2).desired_width(ui.available_width()).margin(egui::Margin::symmetric(8.0, 6.0)));
                        });
                    });
                });

                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "所属项目、承包单位、发包人可直接输入新名称：库里没有会自动建档，项目编码自动生成。",
                    )
                    .size(11.5)
                    .color(p.text_muted),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, &p, "下一步：附件", base_ok).clicked() && base_ok {
                        next = true;
                    }
                    if widgets::ghost_button(ui, &p, "取消").clicked() {
                        cancel = true;
                    }
                    ui.add_space(10.0);
                    ui.scope(|ui| {
                        ui.style_mut().visuals.selection.bg_fill = p.primary;
                        ui.checkbox(
                            &mut app.new_contract.activate,
                            RichText::new("保存后立即生效（可直接拨款）")
                                .size(12.5)
                                .color(p.text),
                        )
                        .on_hover_text("不勾选则保存为草稿，补齐合同附件后可在详情卡点「设为生效」");
                    });
                });
                if !base_ok {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("请补全必填项（名称、项目、单位、甲方、金额、日期、付款条款）")
                            .size(11.5)
                            .color(p.text_muted),
                    );
                }
            } else {
                // ---------------- 第二步：附件（四类分项，每类一份）
                ui.label(
                    RichText::new(
                        "四类附件分项存放，「合同」为必传项，其余三类可选；每类仅一份，重新选择即替换。单份不超过 50 MB。",
                    )
                    .size(12.0)
                    .color(p.text_muted),
                );
                ui.add_space(10.0);

                for kind in AttachmentKind::ALL {
                    let existing = app
                        .new_contract
                        .attachment(kind)
                        .map(|a| (a.file_name.clone(), a.size_label()));
                    egui::Frame::none()
                        .fill(p.surface_alt)
                        .rounding(egui::Rounding::same(9.0))
                        .inner_margin(egui::Margin::symmetric(12.0, 9.0))
                        .show(ui, |ui| {
                            ui.set_width(988.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(kind.label()).size(13.0).strong().color(p.text),
                                );
                                if kind.is_required() {
                                    widgets::badge(ui, &p, "必传", Tone::Danger);
                                }
                                ui.add_space(10.0);
                                match &existing {
                                    Some((name, size)) => {
                                        ui.label(
                                            RichText::new(name).size(12.5).color(p.primary),
                                        );
                                        ui.label(
                                            RichText::new(size).size(11.5).color(p.text_muted),
                                        );
                                    }
                                    None => {
                                        ui.label(
                                            RichText::new("未上传").size(12.5).color(p.text_muted),
                                        );
                                    }
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if existing.is_some() {
                                            if widgets::ghost_button_sm(ui, &p, "移除").clicked()
                                            {
                                                remove = Some(kind);
                                            }
                                            if widgets::ghost_button_sm(ui, &p, "替换").clicked()
                                            {
                                                pick = Some(kind);
                                            }
                                        } else if widgets::ghost_button_sm(ui, &p, "选择文件")
                                            .clicked()
                                        {
                                            pick = Some(kind);
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(7.0);
                }

                if let Some(msg) = &app.new_contract_flash {
                    ui.add_space(2.0);
                    ui.label(RichText::new(msg).size(11.5).color(p.danger));
                }
                if activate && !has_contract_copy {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(
                            "已勾选「保存后立即生效」：请先上传合同附件，或取消勾选先存为草稿。",
                        )
                        .size(11.5)
                        .color(p.warn),
                    );
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if widgets::ghost_button(ui, &p, "上一步").clicked() {
                        back = true;
                    }
                    if widgets::primary_button(ui, &p, "保存合同", can_save).clicked() && can_save {
                        submit = true;
                    }
                    if widgets::ghost_button(ui, &p, "取消").clicked() {
                        cancel = true;
                    }
                    ui.add_space(10.0);
                    ui.scope(|ui| {
                        ui.style_mut().visuals.selection.bg_fill = p.primary;
                        ui.checkbox(
                            &mut app.new_contract.activate,
                            RichText::new("保存后立即生效（可直接拨款）")
                                .size(12.5)
                                .color(p.text),
                        );
                    });
                });
            }
        });

    // ---------------- 交互落地
    if cancel {
        app.dialog = Dialog::None;
        app.new_contract = ContractForm::default();
        app.new_contract_flash = None;
        return;
    }
    if next {
        app.new_contract.step = 1;
        app.new_contract_flash = None;
    }
    if back {
        app.new_contract.step = 0;
        app.new_contract_flash = None;
    }
    if let Some(kind) = remove {
        app.new_contract.attachments.retain(|a| a.kind != kind);
        app.new_contract_flash = None;
    }
    if let Some(kind) = pick {
        match pick_attachment(kind) {
            Ok(att) => {
                app.new_contract.attachments.retain(|a| a.kind != kind);
                app.new_contract.attachments.push(att);
                app.new_contract_flash = None;
            }
            Err(e) => {
                // 用户点「取消」不算错误，不打扰
                if e != "已取消选择" {
                    flash = Some(e);
                }
            }
        }
        if let Some(msg) = flash {
            app.new_contract_flash = Some(msg);
        }
    }

    if submit {
        let f = &app.new_contract;
        let project_text = f.project_text.trim().to_string();
        let contractor_text = f.contractor_text.trim().to_string();
        let party_a = f.party_a.trim().to_string();
        let name = f.name.trim().to_string();
        let amount = Money::parse_yuan(&f.amount).unwrap_or(Money::ZERO);
        let ctype = CONTRACT_TYPES
            .get(f.ctype_idx)
            .copied()
            .unwrap_or(ContractType::Construct);
        let cat_code = dict::CONTRACT_CATEGORIES
            .get(f.category_idx)
            .map(|(c, _)| c.to_string())
            .unwrap_or_default();
        let pricing = match f.pricing_idx {
            0 => None,
            1 => Some(ctp_core::contract::PricingMethod::LumpSum),
            2 => Some(ctp_core::contract::PricingMethod::UnitPrice),
            _ => Some(ctp_core::contract::PricingMethod::CostPlus),
        };
        let duration = f.duration.trim().parse::<i64>().unwrap_or(0);
        let retention = pct_to_bp(&f.retention);
        let sign_date = f.sign_date.trim().to_string();
        let planned_start = opt(&f.planned_start);
        let planned_end = opt(&f.planned_end);
        let delivery_date = opt(&f.delivery_date);
        let terms = f.terms.trim().to_string();
        let remark = opt(&f.remark);
        let bid_method = match f.bid_method_idx {
            1 => Some(BidMethod::Open),
            2 => Some(BidMethod::Government),
            3 => Some(BidMethod::Direct),
            _ => None,
        };
        let open_bid_date = opt(&f.open_bid_date);
        let project_leader = opt(&f.project_leader);
        let contact_info = opt(&f.contact_info);
        let bid_agent_text = opt(&f.bid_agent_text);
        let activate = f.activate;
        let atts: Vec<(AttachmentKind, String, Option<String>, Vec<u8>)> = f
            .attachments
            .iter()
            .map(|a| (a.kind, a.file_name.clone(), a.mime.clone(), a.bytes.clone()))
            .collect();

        let created = app.exec(move |conn| {
            let c = service::create_contract(
                conn,
                NewContract {
                    project_id: String::new(),
                    project_text: Some(project_text),
                    contract_name: name,
                    contract_type: ctype,
                    contract_category_code: cat_code,
                    // 会计分类字段已从界面移除，新建一律留空
                    category_id: None,
                    party_a,
                    contractor_id: String::new(),
                    contractor_text: Some(contractor_text),
                    building_id: None,
                    sign_date,
                    original_amount: amount,
                    duration_days: duration,
                    planned_start,
                    planned_end,
                    delivery_date,
                    pricing_method: pricing,
                    retention_rate_bp: retention,
                    // 预付款率已从新建页移除，新合同统一按 0 处理
                    advance_rate_bp: 0,
                    payment_terms: terms,
                    remark,
                    bid_method,
                    open_bid_date,
                    project_leader,
                    contact_info,
                    bid_agent_text,
                },
                "界面用户",
            )?;
            for (kind, fname, mime, bytes) in &atts {
                service::save_attachment(
                    conn,
                    &c.id,
                    *kind,
                    fname,
                    mime.as_deref(),
                    bytes,
                    "界面用户",
                )?;
            }
            Ok(c)
        });

        match created {
            Ok(c) => {
                let cid = c.id.clone();
                let cno = c.contract_no.clone();
                let act_err = if activate {
                    app.exec(|conn| service::activate_contract(conn, &cid, "界面用户"))
                        .err()
                } else {
                    None
                };
                app.refresh();
                app.select(&cid);
                app.new_contract = ContractForm::default();
                app.new_contract_flash = None;
                app.dialog = Dialog::None;
                match act_err {
                    None if activate => {
                        app.success(format!("合同 {cno} 已建立并生效，可直接登记拨款"))
                    }
                    None => app.success(format!(
                        "合同 {cno} 已保存为草稿，补齐合同附件后可在详情卡设为生效"
                    )),
                    Some(e) => app.error(format!(
                        "合同 {cno} 已建立，但生效失败：{e}（当前为草稿，可在详情卡重试）"
                    )),
                }
            }
            Err(e) => app.error(format!("新建合同失败：{e}")),
        }
    }
}
// ---------------------------------------------------------------- 合同信息修改（R-50）

/// 合同台账右侧栏「信息修改」：改主档字段，保存时写审计日志。
///
/// 刻意与「新建合同」分成两套表单：新建要一次填齐、分两步、可带附件；
/// 修改是局部订正，一屏平铺更快，也避免误改到金额 / 编号这类敏感字段。
fn edit_contract(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;

    if app.edit_contract.is_none() {
        app.dialog = Dialog::None;
        return;
    }

    // 联想候选先克隆：闭包里要可变借用表单，不能再读 `app.projects`
    let project_src = project_sources(&app.projects);
    let project_hints: Vec<String> = project_src.iter().map(|(l, _)| l.clone()).collect();
    let mut unit_hints: Vec<String> = app
        .contractors
        .iter()
        .filter(|c| c.role_type != ContractorRole::Owner)
        .map(|c| c.contractor_name.clone())
        .collect();
    unit_hints.sort();
    unit_hints.dedup();
    let mut owner_hints: Vec<String> = app
        .contractors
        .iter()
        .filter(|c| c.role_type == ContractorRole::Owner)
        .map(|c| c.contractor_name.clone())
        .collect();
    owner_hints.extend(app.party_a_names.iter().cloned());
    owner_hints.sort();
    owner_hints.dedup();
    let mut agent_hints: Vec<String> = app
        .contractors
        .iter()
        .filter(|c| c.role_type == ContractorRole::Agent)
        .map(|c| c.contractor_name.clone())
        .collect();
    if agent_hints.is_empty() {
        agent_hints = app
            .contractors
            .iter()
            .map(|c| c.contractor_name.clone())
            .collect();
    }
    agent_hints.sort();
    agent_hints.dedup();

    let contract_no = app
        .edit_contract
        .as_ref()
        .map(|f| f.contract_no.clone())
        .unwrap_or_default();
    let contract_id = app
        .edit_contract
        .as_ref()
        .map(|f| f.contract_id.clone())
        .unwrap_or_default();

    // 保存按钮的可用性：与保存时的校验共用同一套条件
    let can_save = {
        let f = app.edit_contract.as_ref().expect("上面已判空");
        !f.name.trim().is_empty()
            && !f.project_text.trim().is_empty()
            && !f.contractor_text.trim().is_empty()
            && !f.party_a.trim().is_empty()
            && is_iso_date(&f.sign_date)
            && !f.terms.trim().is_empty()
            && !f.reason.trim().is_empty()
    };

    let mut submit: Option<(EditContract, String)> = None;
    let mut cancel = false;

    let Some(form) = app.edit_contract.as_mut() else {
        app.dialog = Dialog::None;
        return;
    };

    modal("修改合同信息", &p)
        .default_width(720.0)
        .min_width(720.0)
        .show(ctx, |ui| {
            ui.set_width(700.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("合同编号 {contract_no}"))
                        .size(12.0)
                        .color(p.text_muted),
                );
                ui.label(
                    RichText::new(
                        "编号与金额不在此修改；金额请用「编辑金额」，审计价请用「登记审计价」",
                    )
                    .size(10.5)
                    .color(p.text_muted),
                );
            });
            ui.add_space(8.0);

            egui::ScrollArea::vertical()
                .id_salt("edit_contract_scroll")
                .max_height(520.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    form_card(&p).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        widgets::section_title(ui, &p, "基本信息", "");
                        egui::Grid::new("edit_contract_base")
                            .num_columns(2)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                field(ui, &p, "* 合同名称", 300.0, &mut form.name);
                                ui.vertical(|ui| {
                                    widgets::field_label(ui, &p, "合同类别");
                                    let labels: Vec<String> = dict::CONTRACT_CATEGORIES
                                        .iter()
                                        .map(|(_, n)| n.to_string())
                                        .collect();
                                    combo(
                                        ui,
                                        "edit_category",
                                        300.0,
                                        form.category_idx,
                                        labels,
                                        &mut form.category_idx,
                                    );
                                });
                                ui.end_row();

                                ac_field_pick(
                                    ui,
                                    &p,
                                    "* 所属项目",
                                    "edit_project",
                                    &mut form.project_text,
                                    &project_hints,
                                    "输入编码或名称后联想",
                                    300.0,
                                    widgets::SuggestMode::TypedOnly,
                                    &|s| pick_project_name(&project_src, s),
                                );
                                ac_field(
                                    ui,
                                    &p,
                                    "* 承包单位",
                                    "edit_contractor",
                                    &mut form.contractor_text,
                                    &unit_hints,
                                    "输入名称后联想已有单位",
                                    300.0,
                                    widgets::SuggestMode::TypedOnly,
                                );
                                ui.end_row();

                                ac_field(
                                    ui,
                                    &p,
                                    "* 发包人（甲方）",
                                    "edit_party_a",
                                    &mut form.party_a,
                                    &owner_hints,
                                    "输入名称后联想已有单位",
                                    300.0,
                                    widgets::SuggestMode::TypedOnly,
                                );
                                ui.vertical(|ui| {
                                    widgets::field_label(ui, &p, "合同类型");
                                    let labels: Vec<String> = CONTRACT_TYPES
                                        .iter()
                                        .map(|t| t.label().to_string())
                                        .collect();
                                    combo(
                                        ui,
                                        "edit_ctype",
                                        300.0,
                                        form.ctype_idx,
                                        labels,
                                        &mut form.ctype_idx,
                                    );
                                });
                                ui.end_row();

                                date_field(
                                    ui,
                                    &p,
                                    "* 签订日期",
                                    "edit_sign_date",
                                    300.0,
                                    &mut form.sign_date,
                                );
                                field(ui, &p, "工期（日历天）", 300.0, &mut form.duration);
                                ui.end_row();

                                date_field(
                                    ui,
                                    &p,
                                    "计划开工",
                                    "edit_planned_start",
                                    300.0,
                                    &mut form.planned_start,
                                );
                                // R-50：与新建页同一套规则（含开工当日、手改后停止同步）
                                auto_planned_end(
                                    &form.planned_start,
                                    &form.duration,
                                    &mut form.planned_end,
                                    &mut form.planned_end_auto,
                                );
                                let end_manual = planned_end_is_manual(
                                    &form.planned_end,
                                    &form.planned_end_auto,
                                );
                                let show_note = form.planned_end_dirty && end_manual;
                                let end_changed = date_field_note(
                                    ui,
                                    &p,
                                    "计划交工",
                                    "edit_planned_end",
                                    300.0,
                                    &mut form.planned_end,
                                    if show_note {
                                        Some("已手动修改，不再随工期自动同步")
                                    } else {
                                        None
                                    },
                                );
                                if end_changed {
                                    form.planned_end_dirty = true;
                                }
                                ui.end_row();
                            });

                        ui.add_space(8.0);
                        widgets::section_title(ui, &p, "招标与条款", "");
                        egui::Grid::new("edit_contract_bid")
                            .num_columns(2)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    widgets::field_label(ui, &p, "招标方式");
                                    let labels: Vec<String> = vec![
                                        "（未选）".into(),
                                        "公开招标".into(),
                                        "政府采购".into(),
                                        "直接发包".into(),
                                    ];
                                    combo(
                                        ui,
                                        "edit_bid_method",
                                        300.0,
                                        form.bid_method_idx,
                                        labels,
                                        &mut form.bid_method_idx,
                                    );
                                });
                                date_field(
                                    ui,
                                    &p,
                                    "开标时间",
                                    "edit_open_bid",
                                    300.0,
                                    &mut form.open_bid_date,
                                );
                                ui.end_row();

                                ac_field(
                                    ui,
                                    &p,
                                    "招标代理",
                                    "edit_agent",
                                    &mut form.bid_agent_text,
                                    &agent_hints,
                                    "留空表示清空代理",
                                    300.0,
                                    widgets::SuggestMode::TypedOnly,
                                );
                                ui.vertical(|ui| {
                                    widgets::field_label(ui, &p, "计价方式");
                                    let labels: Vec<String> =
                                        PRICING.iter().map(|x| x.to_string()).collect();
                                    combo(
                                        ui,
                                        "edit_pricing",
                                        300.0,
                                        form.pricing_idx,
                                        labels,
                                        &mut form.pricing_idx,
                                    );
                                });
                                ui.end_row();

                                field(ui, &p, "质保金率 %", 300.0, &mut form.retention);
                                field(ui, &p, "项目负责人", 300.0, &mut form.project_leader);
                                ui.end_row();

                                field(ui, &p, "联系方式", 300.0, &mut form.contact_info);
                                field(ui, &p, "备注", 300.0, &mut form.remark);
                                ui.end_row();
                            });

                        ui.add_space(6.0);
                        widgets::field_label(ui, &p, "付款条款");
                        ui.add_sized(
                            [ui.available_width(), 56.0],
                            egui::TextEdit::multiline(&mut form.terms)
                                .margin(egui::Margin::symmetric(8.0, 6.0)),
                        );
                    });

                    ui.add_space(10.0);
                    // 修改原因必填：与「编辑金额」口径一致
                    form_card(&p).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.vertical(|ui| {
                            widgets::field_label(ui, &p, "* 修改原因（写入操作日志）");
                            ui.add_sized(
                                [ui.available_width(), 26.0],
                                egui::TextEdit::singleline(&mut form.reason)
                                    .hint_text("例如：合同名称笔误更正")
                                    .margin(egui::Margin::symmetric(8.0, 4.0)),
                            );
                        });
                    });

                    if let Some(err) = &form.error {
                        ui.add_space(6.0);
                        ui.label(RichText::new(err).size(11.5).color(p.danger));
                    }
                });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &p, "保存修改", can_save).clicked() && can_save {
                    let ctype = CONTRACT_TYPES
                        .get(form.ctype_idx)
                        .copied()
                        .unwrap_or(ContractType::Construct);
                    let cat_code = dict::CONTRACT_CATEGORIES
                        .get(form.category_idx)
                        .map(|(c, _)| c.to_string())
                        .unwrap_or_default();
                    let pricing = match form.pricing_idx {
                        0 => None,
                        1 => Some(PricingMethod::LumpSum),
                        2 => Some(PricingMethod::UnitPrice),
                        _ => Some(PricingMethod::CostPlus),
                    };
                    let bid_method = match form.bid_method_idx {
                        1 => Some(BidMethod::Open),
                        2 => Some(BidMethod::Government),
                        3 => Some(BidMethod::Direct),
                        _ => None,
                    };
                    submit = Some((
                        EditContract {
                            contract_name: form.name.trim().to_string(),
                            project_text: form.project_text.trim().to_string(),
                            contractor_text: form.contractor_text.trim().to_string(),
                            party_a: form.party_a.trim().to_string(),
                            contract_type: ctype,
                            contract_category_code: cat_code,
                            sign_date: form.sign_date.trim().to_string(),
                            duration_days: form.duration.trim().parse::<i64>().unwrap_or(0),
                            planned_start: opt(&form.planned_start),
                            planned_end: opt(&form.planned_end),
                            pricing_method: pricing,
                            retention_rate_bp: pct_to_bp(&form.retention),
                            payment_terms: form.terms.trim().to_string(),
                            remark: opt(&form.remark),
                            bid_method,
                            open_bid_date: opt(&form.open_bid_date),
                            project_leader: opt(&form.project_leader),
                            contact_info: opt(&form.contact_info),
                            bid_agent_text: opt(&form.bid_agent_text),
                        },
                        form.reason.trim().to_string(),
                    ));
                }
                if widgets::ghost_button(ui, &p, "取消").clicked() {
                    cancel = true;
                }
            });
        });

    if cancel {
        app.dialog = Dialog::None;
        app.edit_contract = None;
        return;
    }

    if let Some((input, reason)) = submit {
        // 弹窗里要显示原因，但 reason 会被 move 进闭包，先留一份
        let reason_text = reason.clone();
        match app.exec(move |conn| {
            service::update_contract_master(conn, &contract_id, input, &reason, "界面用户")
        }) {
            Ok(outcome) => {
                app.dialog = Dialog::None;
                app.edit_contract = None;
                app.refresh();
                app.select(&outcome.contract.id);
                // 成功不再是 6 秒绿条：改成弹窗，把「改了哪些字段、原来是什么」列清楚
                app.edit_done = Some(EditDoneView {
                    contract_no: outcome.contract.contract_no.clone(),
                    contract_name: outcome.contract.contract_name.clone(),
                    reason: reason_text,
                    changes: outcome
                        .changes
                        .iter()
                        .map(|c| EditDoneRow {
                            field: c.field.to_string(),
                            old: c.old.clone(),
                            new: c.new.clone(),
                        })
                        .collect(),
                });
            }
            Err(e) => {
                if let Some(f) = app.edit_contract.as_mut() {
                    f.error = Some(e);
                }
            }
        }
    }
}
/// 「本次修改了什么」弹窗的数据（R-51）。
///
/// 与操作日志同源：日志记 `EDIT_CONTRACT`，弹窗把同一份 diff 逐条列给用户看。
/// 长文本（付款条款 / 备注）在这里显示全文，日志里仍只记「已修改」。
#[derive(Clone)]
pub struct EditDoneView {
    pub contract_no: String,
    pub contract_name: String,
    pub reason: String,
    pub changes: Vec<EditDoneRow>,
}

/// 弹窗里的一行：字段名 + 原值 + 新值。
#[derive(Clone)]
pub struct EditDoneRow {
    pub field: String,
    pub old: String,
    pub new: String,
}

impl EditDoneView {
    /// 截图核对用的样例（`CTP_DIALOG=edit_done` 时构造），不参与业务逻辑。
    ///
    /// 里面故意放一条长文本备注，用来核对折行与滚动是否正常。
    pub fn sample(contract_no: &str, contract_name: &str) -> Self {
        let row = |f: &str, o: &str, n: &str| EditDoneRow {
            field: f.into(),
            old: o.into(),
            new: n.into(),
        };
        Self {
            contract_no: contract_no.into(),
            contract_name: contract_name.into(),
            reason: "项目归属更正".into(),
            changes: vec![
                row("所属项目", "测试项目", "另一个项目"),
                row("合同名称", contract_name, "（更名后的合同）"),
                row("工期", "90 日历天", "120 日历天"),
                row("质保金率", "3.00%", "3.50%"),
                row("计划交工", "2026-05-29", "2026-06-28"),
                row("备注", "—", "补充说明：本合同为二期扩建配套工程，含场平、管网及临时道路，付款按季度计量，质保金比例 3%，缺陷责任期 24 个月。"),
            ],
        }
    }
}

/// 保存成功后的「本次修改」弹窗。
///
/// 是叠加层，不占用 `Dialog`；只认「关闭」按钮，Esc 与点外部都不关，
/// 保证用户至少看一眼自己改了什么。
fn edit_done_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(view) = app.edit_done.clone() else {
        return;
    };
    let p = app.pal;
    let mut close = false;
    egui::Window::new("合同信息已修改")
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .frame(
            egui::Frame::none()
                .fill(p.surface)
                .stroke(egui::Stroke::new(1.0_f32, p.border))
                .rounding(egui::Rounding::same(14.0))
                .inner_margin(egui::Margin::same(20.0)),
        )
        .show(ctx, |ui| {
            ui.set_width(660.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} {}", view.contract_no, view.contract_name))
                        .size(14.5)
                        .strong()
                        .color(p.text),
                );
                widgets::badge(ui, &p, "已写入操作日志", Tone::Success);
            });
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!(
                    "本次共修改 {} 个字段，明细如下（原值 → 新值）",
                    view.changes.len()
                ))
                .size(12.0)
                .color(p.text_muted),
            );
            ui.add_space(8.0);
            widgets::divider(ui, &p);
            ui.add_space(8.0);

            let head = |ui: &mut egui::Ui, w: f32, text: &str| {
                ui.allocate_ui_with_layout(
                    egui::vec2(w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(w);
                        ui.set_max_width(w);
                        ui.label(RichText::new(text).size(11.5).strong().color(p.text_muted));
                    },
                );
            };
            let (w_field, w_val) = (104.0, 230.0);
            egui::ScrollArea::vertical()
                .id_salt("edit_done_scroll")
                .max_height(320.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("edit_done_grid")
                        .num_columns(3)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            head(ui, w_field, "字段");
                            head(ui, w_val, "修改前");
                            head(ui, w_val, "修改后");
                            ui.end_row();
                            for row in &view.changes {
                                cell_text(ui, w_field, &row.field, p.text, true);
                                // 长文本按列宽换行，不裁切也不撑破表格
                                cell_text(ui, w_val, &row.old, p.text_muted, false);
                                cell_text(ui, w_val, &row.new, p.text, false);
                                ui.end_row();
                            }
                        });
                });

            ui.add_space(10.0);
            widgets::soft_box(&p, p.primary_soft).show(ui, |ui| {
                ui.set_width(620.0);
                ui.label(
                    RichText::new("修改原因")
                        .size(11.5)
                        .strong()
                        .color(p.primary),
                );
                ui.add_space(2.0);
                ui.label(RichText::new(&view.reason).size(12.5).color(p.text));
            });

            ui.add_space(10.0);
            widgets::divider(ui, &p);
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "关闭方式：点击下方「关闭」返回台账；按 Esc 或点击弹窗外部不会关闭。修改已留痕，可在「操作日志」按 EDIT_CONTRACT 复查。",
                )
                .size(11.0)
                .color(p.text_muted),
            );
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::primary_button(ui, &p, "关闭", true).clicked() {
                    close = true;
                }
            });
        });

    if close {
        app.edit_done = None;
    }
}

/// 定宽换行文本单元：值短时只占一行，长文本（付款条款 / 备注）自动折行。
///
/// `allocate_ui_with_layout` 默认会把宽度收缩到内容宽度，表格三列就会各按内容
/// 自适应、列头与数值对不齐；这里显式钉住最小/最大宽度，列宽才是稳定的。
fn cell_text(ui: &mut egui::Ui, w: f32, text: &str, color: egui::Color32, strong: bool) {
    ui.allocate_ui_with_layout(
        egui::vec2(w, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_width(w);
            ui.set_max_width(w);
            let mut txt = RichText::new(if text.is_empty() { "—" } else { text })
                .size(12.0)
                .color(color);
            if strong {
                txt = txt.strong();
            }
            ui.add(egui::Label::new(txt).wrap());
        },
    );
}

// ---------------------------------------------------------------- 项目交工

/// 项目交工：选一个日期写入该合同的「交工日期」。
fn delivery_date(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    let Some(cid) = app.selected.clone() else {
        app.dialog = Dialog::None;
        return;
    };
    let cno = app
        .detail
        .as_ref()
        .map(|d| d.contract_no.clone())
        .unwrap_or_default();

    let mut close = false;
    let mut submit = false;
    modal(format!("项目交工 · {cno}"), &p)
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(
                RichText::new(
                    "登记该合同的交工日期；确认后可在合同台账的「交工日期」列与详情卡查看。",
                )
                .size(12.0)
                .color(p.text_muted),
            );
            ui.add_space(10.0);
            widgets::field_label(ui, &p, "* 交工日期");
            widgets::date_input(
                ui,
                "delivery_date_dialog",
                &p,
                &mut app.delivery_form,
                260.0,
            );

            let date_ok = is_iso_date(&app.delivery_form);
            // R-52：与已登记的交工日期一致时没有可留痕的变化，不用提交后再报一次
            let cur_delivery = app.detail.as_ref().and_then(|c| c.delivery_date.clone());
            let same_date = cur_delivery
                .as_deref()
                .map(|d| d.trim() == app.delivery_form.trim())
                .unwrap_or(false);
            let ok = date_ok && !same_date;
            if !date_ok {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("请选择合法日期（YYYY-MM-DD）")
                        .size(11.5)
                        .color(p.warn),
                );
            } else if same_date {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("与已登记的交工日期一致，无需重复提交")
                        .size(11.5)
                        .color(p.warn),
                );
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &p, "确认交工", ok).clicked() && ok {
                    submit = true;
                }
                if widgets::ghost_button(ui, &p, "取消").clicked() {
                    close = true;
                }
            });
        });

    if close {
        app.dialog = Dialog::None;
    }
    if submit {
        let date = app.delivery_form.trim().to_string();
        match app.exec(move |conn| service::set_delivery_date(conn, &cid, &date, "界面用户")) {
            Ok(c) => {
                app.refresh();
                app.select(&c.id);
                app.success(format!(
                    "交工日期已登记：{}",
                    c.delivery_date.unwrap_or_default()
                ));
                app.dialog = Dialog::None;
            }
            Err(e) => app.error(format!("登记交工日期失败：{e}")),
        }
    }
}

/// 表单文本与库里的可选文本是否同值（两端 trim，空串等价于未填）。
fn opt_text_eq(form: &str, db: &Option<String>) -> bool {
    form.trim() == db.as_deref().unwrap_or("").trim()
}

/// 审计价弹窗预填的就是现值：四个字段都没改动时不必提交（R-52 / P3）。
fn audit_form_unchanged(app: &App) -> bool {
    let Some(c) = app.detail.as_ref() else {
        return false;
    };
    let typed = Money::parse_yuan(&app.audit_form.amount)
        .ok()
        .filter(|m| m.cent() > 0);
    typed == c.audit_price()
        && opt_text_eq(&app.audit_form.date, &c.audit_date)
        && opt_text_eq(&app.audit_form.org, &c.audit_org)
        && opt_text_eq(&app.audit_form.report_no, &c.audit_report_no)
}

fn audit_price(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    let mut cancel = false;
    let mut submit = false;
    let id = app.selected.clone();
    let no = app
        .selected_summary()
        .map(|s| s.contract_no.clone())
        .unwrap_or_default();
    let base = app
        .selected_summary()
        .map(|s| s.base_amount.format_yuan())
        .unwrap_or_default();

    modal(format!("登记审计价 · {no}"), &p)
        .default_width(660.0)
        .show(ctx, |ui| {
            ui.set_width(620.0);
            ui.label(
                RichText::new(format!(
                    "登记后有效计价基准切换为审计价（当前基准 {base} 元）；历史拨款流水不重算。日期格式 YYYY-MM-DD。"
                ))
                .size(12.0)
                .color(p.text_muted),
            );
            ui.add_space(10.0);
            egui::Grid::new("audit_form")
                .num_columns(2)
                .spacing([14.0, 10.0])
                .show(ui, |ui| {
                    field(ui, &p, "* 审计价（元）", 300.0, &mut app.audit_form.amount);
                    date_field(ui, &p, "审定日期", "audit_date", 300.0, &mut app.audit_form.date);
                    ui.end_row();
                    field(ui, &p, "审定单位", 300.0, &mut app.audit_form.org);
                    field(ui, &p, "审定报告号", 300.0, &mut app.audit_form.report_no);
                    ui.end_row();
                });
            ui.add_space(6.0);
            widgets::field_label(ui, &p, "* 变更原因（写入留痕）");
            ui.add_sized(
                [620.0, 30.0],
                egui::TextEdit::singleline(&mut app.audit_form.reason)
                    .margin(egui::Margin::symmetric(8.0, 4.0)),
            );

            let amount_ok = Money::parse_yuan(&app.audit_form.amount)
                .map(|m| m.cent() > 0)
                .unwrap_or(false);
            let date_ok = app.audit_form.date.trim().is_empty()
                || is_iso_date(&app.audit_form.date);
            let reason_ok = !app.audit_form.reason.trim().is_empty();
            // R-52：预填的就是现值，四个字段都没改 → 不提交，免得写重复历史与日志
            let unchanged = audit_form_unchanged(app);
            let ok = amount_ok && date_ok && reason_ok && id.is_some() && !unchanged;
            if unchanged {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("审计价信息与当前档案一致，无需重复登记")
                        .size(11.5)
                        .color(p.warn),
                );
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &p, "确认登记", ok).clicked() && ok {
                    submit = true;
                }
                if widgets::ghost_button(ui, &p, "取消").clicked() {
                    cancel = true;
                }
            });
        });

    if cancel {
        app.dialog = Dialog::None;
    }
    if submit {
        if let Some(id) = id {
            let price = Money::parse_yuan(&app.audit_form.amount).ok();
            let date = {
                let d = app.audit_form.date.trim().to_string();
                if d.is_empty() {
                    None
                } else {
                    Some(d)
                }
            };
            let org = opt(&app.audit_form.org);
            let report = opt(&app.audit_form.report_no);
            let reason = app.audit_form.reason.trim().to_string();
            let res = app.exec(move |conn| {
                service::register_audit_price(
                    conn,
                    &id,
                    price,
                    date,
                    org,
                    report,
                    &reason,
                    "界面用户",
                )
            });
            match res {
                Ok(c) => {
                    app.refresh();
                    app.audit_form = AuditForm::default();
                    app.dialog = Dialog::None;
                    app.success(format!(
                        "审计价已登记，有效计价基准 {} 元",
                        c.effective_base().format_yuan()
                    ));
                }
                Err(e) => app.error(format!("登记审计价失败：{e}")),
            }
        }
    }
}

// ---------------------------------------------------------------- 金额编辑

fn edit_amount(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    let mut cancel = false;
    let mut submit = false;
    let id = app.selected.clone();
    let (no, cur, cur_money) = app
        .selected_summary()
        .map(|s| {
            (
                s.contract_no.clone(),
                s.total_amount.format_yuan(),
                s.total_amount,
            )
        })
        .unwrap_or_default();

    modal(format!("修改合同金额 · {no}"), &p)
        .default_width(480.0)
        .show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(
                RichText::new(format!(
                    "当前合同金额 {cur} 元。修改会留痕（BR-22）；已审定合同不改基准。"
                ))
                .size(12.0)
                .color(p.text_muted),
            );
            ui.add_space(10.0);
            field(
                ui,
                &p,
                "* 新合同金额（元）",
                440.0,
                &mut app.amount_form.amount,
            );
            ui.add_space(6.0);
            widgets::field_label(ui, &p, "* 修改原因");
            ui.add_sized(
                [440.0, 30.0],
                egui::TextEdit::singleline(&mut app.amount_form.reason)
                    .margin(egui::Margin::symmetric(8.0, 4.0)),
            );
            let amount_ok = Money::parse_yuan(&app.amount_form.amount)
                .map(|m| m.cent() > 0)
                .unwrap_or(false);
            // R-52：新金额与当前一致时没有可留痕的变化，别让用户提交后再吃一次提示
            let same_amount = Money::parse_yuan(&app.amount_form.amount).ok() == Some(cur_money);
            let ok = amount_ok
                && !app.amount_form.reason.trim().is_empty()
                && id.is_some()
                && !same_amount;
            if same_amount {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("新金额与当前金额一致，无需保存")
                        .size(11.5)
                        .color(p.warn),
                );
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &p, "保存", ok).clicked() && ok {
                    submit = true;
                }
                if widgets::ghost_button(ui, &p, "取消").clicked() {
                    cancel = true;
                }
            });
        });

    if cancel {
        app.dialog = Dialog::None;
    }
    if submit {
        if let Some(id) = id {
            let amount = Money::parse_yuan(&app.amount_form.amount).unwrap_or(Money::ZERO);
            let reason = app.amount_form.reason.trim().to_string();
            let res = app.exec(move |conn| {
                service::edit_contract_amount(conn, &id, amount, &reason, "界面用户")
            });
            match res {
                Ok(c) => {
                    app.refresh();
                    app.amount_form = AmountForm::default();
                    app.dialog = Dialog::None;
                    app.success(format!(
                        "合同金额已改为 {} 元",
                        c.total_amount().format_yuan()
                    ));
                }
                Err(e) => app.error(format!("修改失败：{e}")),
            }
        }
    }
}

// ---------------------------------------------------------------- 拨款单详情

/// 只读的拨款单详情：从总览「最近拨款」或拨款明细点单号进来。
fn payment_detail(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    let Some(pm) = app.pay_detail.clone() else {
        app.dialog = Dialog::None;
        return;
    };
    let contract = app
        .summaries
        .iter()
        .find(|s| s.contract_id == pm.contract_id)
        .cloned();
    let voided = pm.status == ctp_core::payment::PaymentStatus::Void;
    let created = pm
        .created_at
        .get(0..16)
        .map(|s| s.replace('T', " "))
        .unwrap_or_else(|| pm.created_at.clone());
    let mut close = false;
    let mut open_contract = false;
    let mut print = false;

    modal(format!("拨款单 {}", pm.payment_no), &p)
        .default_width(560.0)
        .show(ctx, |ui| {
            ui.set_width(520.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} 元", pm.amount_cent.format_yuan()))
                        .size(24.0)
                        .strong()
                        .color(if voided { p.text_muted } else { p.text }),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if voided {
                        widgets::badge(ui, &p, "作废", Tone::Neutral);
                    } else {
                        widgets::badge(ui, &p, "已付", Tone::Success);
                    }
                });
            });
            ui.label(
                RichText::new(if voided {
                    "作废单保留记录，不计入已付净额。"
                } else {
                    "计入该合同的累计已付。"
                })
                .size(11.5)
                .color(p.text_muted),
            );

            ui.add_space(8.0);
            widgets::divider(ui, &p);
            widgets::kv(
                ui,
                &p,
                "合同编号",
                contract
                    .as_ref()
                    .map(|c| c.contract_no.as_str())
                    .unwrap_or("—"),
            );
            widgets::kv(
                ui,
                &p,
                "合同名称",
                contract
                    .as_ref()
                    .map(|c| c.contract_name.as_str())
                    .unwrap_or("—"),
            );
            widgets::kv(ui, &p, "开单日期", &pm.pay_date);
            widgets::kv(ui, &p, "支付方式", pm.pay_method.label());
            widgets::kv(ui, &p, "事由", pm.reason.as_deref().unwrap_or("—"));
            if let Some(t) = &pm.title {
                widgets::kv(ui, &p, "摘要", t);
            }
            widgets::kv(ui, &p, "经办人", &pm.operator_id);
            widgets::kv(ui, &p, "登记时间", &created);
            if let Some(r) = &pm.remark {
                widgets::kv(ui, &p, "备注", r);
            }

            ui.add_space(6.0);
            widgets::divider(ui, &p);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &p, "关闭", true).clicked() {
                    close = true;
                }
                if !voided && widgets::ghost_button(ui, &p, "打印").clicked() {
                    print = true;
                }
                if contract.is_some() && widgets::ghost_button(ui, &p, "查看合同详情").clicked()
                {
                    open_contract = true;
                }
            });
        });

    if close {
        app.dialog = Dialog::None;
        app.pay_detail = None;
    } else if open_contract {
        // 与合同台账「点击合同编号」走同一个只读详情弹窗
        app.pay_detail = None;
        app.open_contract_detail(&pm.contract_id);
    } else if print {
        app.open_print_preview(&pm);
    }
}

// ---------------------------------------------------------------- 付款打印

/// 付款打印预览：显示按真实版面渲染出来的整页图，确认后直接弹系统打印对话框。
///
/// 与出纸共用同一份版面代码（`ctp_app::paper`），所以预览就是成品。
/// A4 横向、每张单据两页；批量时按列表顺序连续排页。
fn print_preview(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    if app.print_vouchers.is_empty() {
        app.dialog = Dialog::None;
        // R-44：预览意外关闭时一并清掉成功提示，避免留下孤立的倒计时弹窗。
        dismiss_print_success(&mut app.print_success);
        return;
    }
    let sheets = app.print_vouchers.len();
    let total_pages = sheets * ctp_app::paper::PAGES_PER_VOUCHER;
    let page = app.print_page.min(total_pages.saturating_sub(1));
    let first_no = app.print_vouchers[0].payment_no.clone();
    let mut close = false;
    let mut do_print = false;

    // ---- 当前页位图（带缓存，只在换页/首次时渲染）
    // R-42：失败弹窗未关闭、或本页已经渲染失败时不再重试，
    // 避免每帧重复报错 / 重复弹窗；预览区会显示「预览渲染失败」占位。
    if app.print_failure.is_none()
        && !app.print_render_failed
        && app.print_tex.as_ref().map(|(_, i)| *i) != Some(page)
    {
        let rendered = {
            let v = &app.print_vouchers[page / ctp_app::paper::PAGES_PER_VOUCHER];
            let sub = page % ctp_app::paper::PAGES_PER_VOUCHER;
            ctp_app::render_print_page(v, sub, 150.0)
        };
        match rendered {
            Ok(pv) => {
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [pv.width as usize, pv.height as usize],
                    &pv.rgba,
                );
                let tex = ctx.load_texture(
                    format!("print-page-{page}"),
                    img,
                    egui::TextureOptions::LINEAR,
                );
                app.print_tex = Some((tex, page));
            }
            Err(e) => {
                app.print_tex = None;
                app.print_render_failed = true;
                // 预览失败也走统一弹窗；预览窗保留在原页，关掉提示即可直接出纸。
                app.show_print_failure(classify_print_failure(PrintStage::Preview, &e));
            }
        }
    }

    let title = if sheets > 1 {
        format!("批量打印拨款单 · 共 {sheets} 张")
    } else {
        format!("打印拨款单 · {first_no}")
    };

    modal(&title, &p).default_width(824.0).show(ctx, |ui| {
        ui.set_width(784.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("A4 横向 · 每张两页 · 直接送打印机")
                    .size(14.0)
                    .strong()
                    .color(p.text),
            );
            if sheets > 1 {
                widgets::badge(ui, &p, &format!("{sheets} 张 / {total_pages} 页"), Tone::Primary);
            }
        });
        ui.add_space(3.0);
        ui.label(
            RichText::new(format!(
                "第 1 页《财务支出审核监督表》· 第 2 页《工程项目进度款支付申请表》{}",
                if sheets > 1 {
                    "· 多张单据按列表顺序连续排列"
                } else {
                    ""
                }
            ))
            .size(11.5)
            .color(p.text_muted),
        );

        // ---- 翻页条
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let can_prev = page > 0;
            if widgets::ghost_button_sm_on(ui, &p, "上一页", can_prev).clicked() && can_prev {
                app.print_page = page - 1;
                app.print_render_failed = false;
            }
            let can_next = page + 1 < total_pages;
            if widgets::ghost_button_sm_on(ui, &p, "下一页", can_next).clicked() && can_next {
                app.print_page = page + 1;
                app.print_render_failed = false;
            }
            ui.add_space(10.0);
            let sheet_no = page / ctp_app::paper::PAGES_PER_VOUCHER + 1;
            let sub = page % ctp_app::paper::PAGES_PER_VOUCHER + 1;
            let mut txt = format!("第 {} / {} 页", page + 1, total_pages);
            if sheets > 1 {
                txt.push_str(&format!("（第 {sheet_no} 张 · 其第 {sub} 页）"));
            }
            ui.label(RichText::new(txt).size(12.0).color(p.text_muted));
        });

        // ---- 纸张预览
        ui.add_space(6.0);
        egui::Frame::none()
            .fill(p.surface_alt)
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                // A4 横向比例 297:210，宽度取 760 时高度约 537
                ui.add_space(1.0);
                if let Some((tex, _)) = &app.print_tex {
                    ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(760.0, 537.4)));
                } else {
                    widgets::empty_state(ui, &p, "预览渲染失败");
                }
            });

        ui.add_space(8.0);
        widgets::soft_box(&p, p.primary_soft).show(ui, |ui| {
            ui.set_width(764.0);
            ui.label(
                RichText::new(
                    "「打印」直接弹系统打印对话框，由 Windows 驱动出纸，不需要 Word、Edge 或任何外部软件。\
                     纸型须为 A4 横向；若对话框首选项里是纵向，程序会拦下并提示改纸型。\
                     作废单不允许打印；会计分类名、附件张数、日期三处按口径留空待手填。",
                )
                .size(11.5)
                .color(p.primary),
            );
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            // R-42/R-44：任一打印提示未关闭时禁掉「打印」，
            // 与 print_now 里的守卫一起挡住 5 秒内的重复提交。
            if widgets::primary_button(
                ui,
                &p,
                "打印",
                !print_busy(&app.print_failure, &app.print_success),
            )
            .on_hover_text("弹系统打印对话框并输出")
            .clicked()
            {
                do_print = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::ghost_button(ui, &p, "关闭").clicked() {
                    close = true;
                }
            });
        });
    });

    if do_print {
        app.print_now();
    }
    if close {
        app.dialog = Dialog::None;
        app.print_vouchers.clear();
        app.print_ids.clear();
        app.print_tex = None;
        app.print_page = 0;
        app.print_render_failed = false;
        dismiss_print_failure(&mut app.print_failure);
        dismiss_print_success(&mut app.print_success);
    }
}

// ---------------------------------------------------------------- 合同附件

/// 把字节落到临时目录后用系统默认程序打开。
///
/// 附件存在库里，没有「原文件路径」可复用，所以每次打开都先物化一份临时副本。
pub(crate) fn open_external(path: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    let mut cmd = {
        // start 的第一个参数是窗口标题，必须显式给空串，否则带空格的路径会被当成标题
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开失败：{e}"))
}

/// 打印附件材料（预留）：列示四类材料的在位情况。
///
/// 本期只做材料清点与入口预留；中标通知书 / 审计报告等模板渲染待接入，
/// 点击「打印」只给出提示，不生成文件。
/// 只读字段：标签 + 浅底值框，尺寸与新合同页的输入框一致。
fn ro_field(ui: &mut egui::Ui, p: &Palette, label: &str, value: &str, w: f32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        widgets::field_label(ui, p, label);
        egui::Frame::none()
            .fill(p.surface_alt)
            .stroke(egui::Stroke::new(1.0_f32, p.border))
            .rounding(egui::Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
            .show(ui, |ui| {
                ui.set_width((w - 16.0).max(40.0));
                ui.label(RichText::new(value).size(12.0).color(p.text));
            });
    });
}

fn ro_text(ui: &mut egui::Ui, p: &Palette, text: &str) {
    egui::Frame::none()
        .fill(p.surface_alt)
        .stroke(egui::Stroke::new(1.0_f32, p.border))
        .rounding(egui::Rounding::same(6.0))
        .inner_margin(egui::Margin::symmetric(8.0, 6.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).size(12.0).color(p.text));
        });
}

/// 合同详情（只读）：布局与新建合同一致；底部三个按钮读取并展示对应附件。
fn contract_detail(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    let Some(c) = app.modal_contract.clone() else {
        app.dialog = Dialog::None;
        return;
    };

    let project_text = app
        .projects
        .iter()
        .find(|x| x.id == c.project_id)
        .map(|x| format!("{} {}", x.project_code, x.project_name))
        .unwrap_or_else(|| "—".into());
    let contractor = app
        .contractors
        .iter()
        .find(|x| x.id == c.contractor_id)
        .map(|x| x.contractor_name.clone())
        .unwrap_or_else(|| "—".into());
    let agent = c
        .bid_agent_id
        .as_ref()
        .and_then(|id| app.contractors.iter().find(|x| &x.id == id))
        .map(|x| x.contractor_name.clone())
        .unwrap_or_else(|| "—".into());
    let category = dict::contract_category_name(&c.contract_category_code).unwrap_or("—");
    // 金额与支付率一律取 V1 汇总行（与总览、台账、打印同源），此处不另算一套
    let summary = app
        .summaries
        .iter()
        .find(|s| s.contract_id == c.id)
        .cloned();

    let mut close = false;
    let mut material: Option<AttachmentKind> = None;

    modal(format!("合同详情 · {}", c.contract_no), &p)
        .default_width(960.0)
        .min_width(960.0)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 16.0))
        .show(ctx, |ui| {
            ui.set_width(920.0);
            ui.label(
                RichText::new("只读视图；如需修改请关闭后从对应操作入口处理。")
                    .size(12.0)
                    .color(p.text_muted),
            );
            ui.add_space(8.0);

            // 顶部四项：合同金额 / 审计价 / 累计已付 / 支付率。
            // 支付率分母为有效计价基准（审计价 > 0 取审计价，否则合同金额）。
            // R-30：四张卡只留标签与数值，不带任何说明性副标题——等高才整齐；
            // 超付这类风险改由数值标红（Tone::Danger）表达，不靠文字解释。
            let audit_text = match summary.as_ref().and_then(|s| s.audit_price) {
                Some(v) => v.format_yuan(),
                None => "—".to_string(),
            };
            let paid_text = match &summary {
                Some(s) => s.paid_amount.format_yuan(),
                None => "—".to_string(),
            };
            let (rate_text, rate_tone) = match &summary {
                Some(s) if s.is_inverted => (s.pay_rate.percent_string(), Tone::Danger),
                Some(s) => (s.pay_rate.percent_string(), Tone::Primary),
                None => ("—".to_string(), Tone::Neutral),
            };
            let cards = [
                (
                    "合同金额",
                    c.original_amount_cent.format_yuan(),
                    Tone::Neutral,
                ),
                ("审计价", audit_text, Tone::Neutral),
                ("累计已付", paid_text, Tone::Primary),
                ("支付率", rate_text, rate_tone),
            ];
            let gap = 12.0_f32;
            let cw = (920.0 - gap * 3.0) / 4.0;
            ui.scope(|ui| {
                ui.style_mut().spacing.item_spacing.x = gap;
                ui.horizontal(|ui| {
                    for (label, value, tone) in cards {
                        widgets::kpi(ui, &p, cw, label, value.as_str(), "", tone);
                    }
                });
            });
            ui.add_space(10.0);

            // 左卡：合同双方 + 招标与工期；右卡：合同属性 + 条款备注。
            // 两块之间用细分隔线区隔，不再画分栏标题。
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(454.0);
                    form_card(&p).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::Grid::new("ro_subject")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ro_field(ui, &p, "合同名称", &c.contract_name, 200.0);
                                ro_field(ui, &p, "所属项目", &project_text, 200.0);
                                ui.end_row();
                                ro_field(ui, &p, "发包人（甲方）", &c.party_a, 200.0);
                                ro_field(ui, &p, "承包单位", &contractor, 200.0);
                                ui.end_row();
                                ro_field(
                                    ui,
                                    &p,
                                    "项目负责人",
                                    c.project_leader.as_deref().unwrap_or("—"),
                                    200.0,
                                );
                                ro_field(
                                    ui,
                                    &p,
                                    "联系方式",
                                    c.contact_info.as_deref().unwrap_or("—"),
                                    200.0,
                                );
                                ui.end_row();
                            });
                        ui.add_space(10.0);
                        widgets::divider(ui, &p);
                        ui.add_space(10.0);
                        egui::Grid::new("ro_bid")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ro_field(
                                    ui,
                                    &p,
                                    "招标方式",
                                    c.bid_method.map(|b| b.label()).unwrap_or("—"),
                                    200.0,
                                );
                                ro_field(
                                    ui,
                                    &p,
                                    "开标时间",
                                    c.open_bid_date.as_deref().unwrap_or("—"),
                                    200.0,
                                );
                                ui.end_row();
                                ro_field(ui, &p, "招标代理", &agent, 200.0);
                                ro_field(
                                    ui,
                                    &p,
                                    "工期（日历天）",
                                    &c.duration_days.to_string(),
                                    200.0,
                                );
                                ui.end_row();
                                ro_field(
                                    ui,
                                    &p,
                                    "计划开工",
                                    c.planned_start.as_deref().unwrap_or("—"),
                                    200.0,
                                );
                                ro_field(
                                    ui,
                                    &p,
                                    "计划交工",
                                    c.planned_end.as_deref().unwrap_or("—"),
                                    200.0,
                                );
                                ui.end_row();
                            });
                    });
                });

                ui.add_space(12.0);

                ui.vertical(|ui| {
                    ui.set_width(454.0);
                    form_card(&p).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::Grid::new("ro_price")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ro_field(ui, &p, "合同类型", c.contract_type.label(), 200.0);
                                ro_field(ui, &p, "合同类别", category, 200.0);
                                ui.end_row();
                                ro_field(ui, &p, "签订日期", &c.sign_date, 200.0);
                                ro_field(
                                    ui,
                                    &p,
                                    "合同金额（元）",
                                    &c.original_amount_cent.format_yuan(),
                                    200.0,
                                );
                                ui.end_row();
                                ro_field(
                                    ui,
                                    &p,
                                    "质保金率 %",
                                    &format!("{:.2}", c.retention_rate_bp as f32 / 100.0),
                                    200.0,
                                );
                                ro_field(
                                    ui,
                                    &p,
                                    "交工日期",
                                    c.delivery_date.as_deref().unwrap_or("—"),
                                    200.0,
                                );
                                ui.end_row();
                            });
                        ui.add_space(10.0);
                        widgets::divider(ui, &p);
                        ui.add_space(10.0);
                        widgets::field_label(ui, &p, "付款条款");
                        ro_text(ui, &p, &c.payment_terms);
                        ui.add_space(6.0);
                        widgets::field_label(ui, &p, "备注");
                        ro_text(ui, &p, c.remark.as_deref().unwrap_or("—"));
                    });
                });
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("附件材料").size(12.0).color(p.text_muted));
                for kind in [
                    AttachmentKind::BidNotice,
                    AttachmentKind::Contract,
                    AttachmentKind::AuditReport,
                ] {
                    if widgets::ghost_button_sm(ui, &p, kind.label()).clicked() {
                        material = Some(kind);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::primary_button(ui, &p, "关闭", true).clicked() {
                        close = true;
                    }
                });
            });
        });

    if close {
        app.dialog = Dialog::None;
        app.modal_contract = None;
        return;
    }

    if let Some(kind) = material {
        let cid = c.id.clone();
        let kind_code = kind.code().to_string();
        let got = app.exec(move |conn| {
            let list = repo::list_attachments(conn, &cid)?;
            let Some(a) = list.into_iter().find(|a| a.kind_code == kind_code) else {
                return Ok(None);
            };
            let bytes = service::attachment_bytes(conn, &a.id)?;
            Ok(Some((a.file_name, bytes)))
        });
        match got {
            Ok(Some((name, bytes))) => {
                let dir = std::env::temp_dir().join("ctp-attachments");
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(&name);
                match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        if let Err(e) = open_external(&path) {
                            app.error(e);
                        }
                    }
                    Err(e) => app.error(format!("写入临时文件失败：{e}")),
                }
            }
            Ok(None) => app.error(format!("「{}」尚未上传", kind.label())),
            Err(e) => app.error(format!("读取附件失败：{e}")),
        }
    }
}

/// 打印附件：**用本机已安装的应用打开原件**，由用户在应用里自行选择打印范围。
///
/// 已确认口径（R-27）：不接管附件的打印，不做合并、不套模板；合同附件多为扫描件
/// （PDF / 图片 / Word），只能交给原应用处理。
fn material_print(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    let cno = app
        .att_contract
        .as_ref()
        .and_then(|id| app.summaries.iter().find(|s| &s.contract_id == id))
        .map(|s| s.contract_no.clone())
        .unwrap_or_else(|| "—".into());

    let mut close = false;
    let mut open_kind: Option<AttachmentKind> = None;

    modal(format!("打印附件材料 · {cno}"), &p)
        .default_width(620.0)
        .show(ctx, |ui| {
            ui.set_width(580.0);
            ui.label(
                RichText::new(
                    "附件按原件打开：用本机已安装的应用（Word / PDF 阅读器等）打开后，\
                     在该应用里选择打印范围出纸。系统不做合并、不套模板、不批量。",
                )
                .size(12.0)
                .color(p.text_muted),
            );
            ui.add_space(10.0);

            for kind in AttachmentKind::ALL {
                let att = app.attachments.iter().find(|a| a.kind_code == kind.code());
                egui::Frame::none()
                    .fill(p.surface_alt)
                    .rounding(egui::Rounding::same(9.0))
                    .inner_margin(egui::Margin::symmetric(12.0, 9.0))
                    .show(ui, |ui| {
                        ui.set_width(556.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(kind.label())
                                    .size(13.0)
                                    .strong()
                                    .color(p.text),
                            );
                            if kind.is_required() {
                                widgets::badge(ui, &p, "必传", Tone::Danger);
                            }
                            ui.add_space(10.0);
                            match att {
                                Some(a) => {
                                    ui.label(
                                        RichText::new(&a.file_name).size(12.0).color(p.primary),
                                    );
                                }
                                None => {
                                    ui.label(
                                        RichText::new("未上传").size(12.0).color(p.text_muted),
                                    );
                                }
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let enabled = att.is_some();
                                    let btn = widgets::ghost_button_sm_on(ui, &p, "打开", enabled);
                                    let btn = if enabled {
                                        btn.on_hover_text("用本机应用打开原件，在应用内打印")
                                    } else {
                                        btn.on_hover_text("该材料未上传，无法打开")
                                    };
                                    if btn.clicked() && enabled {
                                        open_kind = Some(kind);
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(7.0);
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &p, "关闭", true).clicked() {
                    close = true;
                }
            });
        });

    if close {
        app.dialog = Dialog::None;
        return;
    }
    if let Some(kind) = open_kind {
        app.open_attachment_file(kind);
    }
}

fn attachments(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    let Some(cid) = app.att_contract.clone() else {
        app.dialog = Dialog::None;
        return;
    };
    let cno = app
        .summaries
        .iter()
        .find(|s| s.contract_id == cid)
        .map(|s| s.contract_no.clone())
        .unwrap_or_default();

    let mut close = false;
    let mut op_open: Option<Attachment> = None;
    let mut op_save: Option<Attachment> = None;
    let mut op_pick: Option<AttachmentKind> = None;
    let mut op_del: Option<Attachment> = None;

    let list = app.attachments.clone();
    let items: Vec<(AttachmentKind, Option<Attachment>)> = AttachmentKind::ALL
        .iter()
        .map(|k| (*k, list.iter().find(|a| a.kind_code == k.code()).cloned()))
        .collect();

    modal(format!("合同附件 · {cno}"), &p)
        .default_width(760.0)
        .show(ctx, |ui| {
            ui.set_width(720.0);
            ui.label(
                RichText::new("每类一份；「合同」为必传项，未齐备的合同不能设为生效。")
                    .size(12.0)
                    .color(p.text_muted),
            );
            ui.add_space(10.0);

            for (kind, att) in &items {
                egui::Frame::none()
                    .fill(p.surface_alt)
                    .rounding(egui::Rounding::same(9.0))
                    .inner_margin(egui::Margin::symmetric(12.0, 9.0))
                    .show(ui, |ui| {
                        ui.set_width(690.0);
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(158.0, 22.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(kind.label())
                                            .size(13.0)
                                            .strong()
                                            .color(p.text),
                                    );
                                    if kind.is_required() {
                                        widgets::badge(ui, &p, "必传", Tone::Danger);
                                    }
                                },
                            );
                            match att {
                                Some(a) => {
                                    let when = a
                                        .created_at
                                        .get(0..16)
                                        .map(|s| s.replace('T', " "))
                                        .unwrap_or_default();
                                    ui.vertical(|ui| {
                                        ui.label(
                                            RichText::new(&a.file_name).size(12.5).color(p.primary),
                                        );
                                        ui.label(
                                            RichText::new(format!(
                                                "{} · 更新于 {}",
                                                a.size_label(),
                                                when
                                            ))
                                            .size(11.0)
                                            .color(p.text_muted),
                                        );
                                    });
                                }
                                None => {
                                    ui.label(
                                        RichText::new("未上传").size(12.0).color(p.text_muted),
                                    );
                                }
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| match att {
                                    Some(a) => {
                                        if widgets::ghost_button_sm(ui, &p, "删除").clicked() {
                                            op_del = Some(a.clone());
                                        }
                                        if widgets::ghost_button_sm(ui, &p, "另存").clicked() {
                                            op_save = Some(a.clone());
                                        }
                                        if widgets::ghost_button_sm(ui, &p, "打开").clicked() {
                                            op_open = Some(a.clone());
                                        }
                                        if widgets::ghost_button_sm(ui, &p, "替换").clicked() {
                                            op_pick = Some(*kind);
                                        }
                                    }
                                    None => {
                                        if widgets::ghost_button_sm(ui, &p, "上传").clicked() {
                                            op_pick = Some(*kind);
                                        }
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(7.0);
            }

            ui.add_space(6.0);
            widgets::divider(ui, &p);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &p, "关闭", true).clicked() {
                    close = true;
                }
            });
        });

    if close {
        app.dialog = Dialog::None;
        app.att_contract = None;
        app.attachments.clear();
        return;
    }

    if let Some(a) = op_open {
        let id = a.id.clone();
        let name = a.file_name.clone();
        match app.read(move |conn| service::attachment_bytes(conn, &id)) {
            Ok(bytes) => {
                let dir = std::env::temp_dir().join("ctp-attachments");
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(&name);
                if let Err(e) = std::fs::write(&path, &bytes) {
                    app.error(format!("写入临时文件失败：{e}"));
                } else if let Err(e) = open_external(&path) {
                    app.error(e);
                }
            }
            Err(e) => app.error(format!("读取附件失败：{e}")),
        }
    }

    if let Some(a) = op_save {
        let id = a.id.clone();
        let name = a.file_name.clone();
        match app.read(move |conn| service::attachment_bytes(conn, &id)) {
            Ok(bytes) => {
                if let Some(path) = rfd::FileDialog::new().set_file_name(&name).save_file() {
                    match std::fs::write(&path, &bytes) {
                        Ok(()) => app.success(format!("已另存为 {}", path.display())),
                        Err(e) => app.error(format!("另存失败：{e}")),
                    }
                }
            }
            Err(e) => app.error(format!("读取附件失败：{e}")),
        }
    }

    if let Some(kind) = op_pick {
        match pick_attachment(kind) {
            Ok(att) => {
                let cid2 = cid.clone();
                let res = app.exec(move |conn| {
                    service::save_attachment(
                        conn,
                        &cid2,
                        kind,
                        &att.file_name,
                        att.mime.as_deref(),
                        &att.bytes,
                        "界面用户",
                    )
                });
                match res {
                    Ok(_) => {
                        app.reload_attachments();
                        app.success(format!("{} 已更新", kind.label()));
                    }
                    Err(e) => app.error(format!("{} 保存失败：{e}", kind.label())),
                }
            }
            // 用户点「取消」不算错误
            Err(e) if e == "已取消选择" => {}
            Err(e) => app.error(e),
        }
    }

    if let Some(a) = op_del {
        let id = a.id.clone();
        let name = a.file_name.clone();
        match app.exec(move |conn| service::delete_attachment(conn, &id, "界面用户")) {
            Ok(_) => {
                app.reload_attachments();
                app.success(format!("已删除 {name}"));
            }
            Err(e) => app.error(format!("删除失败：{e}")),
        }
    }
}

// ---------------------------------------------------------------- 作废确认

fn confirm_void(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    let mut cancel = false;
    let mut submit = false;

    let (title, line) = match &app.confirm {
        Some(crate::Confirm::VoidPayment(pay)) => (
            "作废拨款单".to_string(),
            format!(
                "{} · {} · {} 元",
                pay.payment_no,
                pay.pay_date,
                pay.amount_cent.format_yuan()
            ),
        ),
        Some(crate::Confirm::VoidContract(c)) => (
            "作废合同".to_string(),
            format!("{} · {}", c.contract_no, c.contract_name),
        ),
        None => return,
    };

    modal(title, &p).default_width(460.0).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.label(RichText::new(line).size(14.0).strong().color(p.text));
        ui.add_space(6.0);
        ui.label(
            RichText::new("作废后记录保留、不计入已付净额（本期以「作废 + 重登」替代红冲）。")
                .size(12.0)
                .color(p.text_muted),
        );
        ui.add_space(10.0);
        widgets::field_label(ui, &p, "* 作废原因");
        ui.add_sized(
            [420.0, 30.0],
            egui::TextEdit::singleline(&mut app.confirm_reason)
                .margin(egui::Margin::symmetric(8.0, 4.0)),
        );
        let ok = !app.confirm_reason.trim().is_empty();
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if widgets::danger_solid_button(ui, &p, "确认作废", ok).clicked() {
                submit = true;
            }
            if widgets::ghost_button(ui, &p, "取消").clicked() {
                cancel = true;
            }
        });
    });

    if cancel {
        app.confirm = None;
        app.confirm_reason.clear();
    }
    if submit {
        let reason = app.confirm_reason.trim().to_string();
        let action = app.confirm.take();
        let res = match action {
            Some(crate::Confirm::VoidPayment(pay)) => {
                let id = pay.id.clone();
                app.exec(move |conn| service::void_payment(conn, &id, &reason, "界面用户"))
                    .map(|_| format!("拨款单 {} 已作废", pay.payment_no))
            }
            Some(crate::Confirm::VoidContract(c)) => {
                let id = c.id.clone();
                let no = c.contract_no.clone();
                app.exec(move |conn| service::void_contract(conn, &id, "界面用户"))
                    .map(|_| format!("合同 {no} 已作废"))
            }
            None => Ok(String::new()),
        };
        match res {
            Ok(msg) => {
                app.refresh();
                app.success(msg);
            }
            Err(e) => app.error(format!("作废失败：{e}")),
        }
        app.confirm_reason.clear();
    }
}

fn opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// 百分比文本 → 万分比（"3.5" → 350 bp）。
fn pct_to_bp(s: &str) -> i32 {
    let t = s.trim();
    if t.is_empty() {
        return 0;
    }
    match t.parse::<f64>() {
        Ok(v) if v.is_finite() => Rate::from_bp((v * 100.0).round() as i32).bp(),
        _ => 0,
    }
}

/// 拨款方式选择项（供拨款页展示当前选中）。
pub fn pay_method_label(idx: usize) -> &'static str {
    PAY_METHODS.get(idx).map(|m| m.label()).unwrap_or("转账")
}

pub fn pay_methods() -> &'static [PayMethod] {
    &PAY_METHODS
}

pub fn new_payment_input(
    contract_id: &str,
    amount: Money,
    date: &str,
    reason: &str,
    idx: usize,
) -> NewPayment {
    let mut input = NewPayment::new(contract_id, amount, date);
    input.reason = Some(reason.to_string());
    input.pay_method = PAY_METHODS.get(idx).copied().unwrap_or(PayMethod::Transfer);
    input
}

/// R-39 回归：导入结果弹窗关闭后，`dialog` 与 `import_result` 必须同时清空。
/// 纯状态变更，不依赖 egui 上下文，可在 `cargo test -p ctp-gui` 中直接断言。
#[cfg(test)]
mod tests {
    use super::*;
    use ctp_app::print::PrintError;
    use std::time::{Duration, Instant};

    fn pv(stage: PrintStage, msg: &str) -> PrintFailureView {
        classify_print_failure(stage, &PrintError::Print(msg.into()))
    }

    #[test]
    fn 关闭导入结果后弹窗与数据都被清除() {
        let mut dialog = Dialog::ImportResult;
        let mut result = Some(ImportResultView::from_report(
            &ctp_app::batch::ImportReport {
                imported: 2,
                skipped: 1,
                failures: vec![],
            },
        ));
        dismiss_import_result(&mut dialog, &mut result);
        assert!(dialog == Dialog::None, "dialog 必须回到 None");
        assert!(
            result.is_none(),
            "import_result 必须清空，避免下次打开残留旧摘要"
        );
    }

    #[test]
    fn 出纸常见失败按类型给出原因和解决方案() {
        // （原始错误, 期望短标签, 原因或建议里必须出现的用户可操作关键词）
        let cases: &[(&str, &str, &str)] = &[
            (
                "当前打印机纸型是纵向（210×297 mm），单据需要 A4 横向",
                "纸张方向不符",
                "横向",
            ),
            (
                "打印对话框失败（公共对话框错误码 2），不是用户取消",
                "打印对话框异常",
                "默认打印机",
            ),
            (
                "打印对话框没有返回打印机句柄",
                "打印对话框异常",
                "默认打印机",
            ),
            (
                "无法开始打印作业（检查打印机是否就绪）",
                "打印机未就绪",
                "打印队列",
            ),
            ("第 3 页输出失败", "出纸中断", "第 3 页"),
            (
                "打印作业结束失败，纸张可能未完整输出，请检查打印机队列后重试",
                "作业未正常结束",
                "打印队列",
            ),
            ("释放打印机句柄失败", "作业未正常结束", "打印队列"),
        ];
        for (msg, kind, hint) in cases {
            let v = pv(PrintStage::Print, msg);
            assert_eq!(v.title, "打印失败");
            assert_eq!(v.kind, *kind, "错误：{msg}");
            assert!(!v.reason.is_empty() && !v.solution.is_empty(), "{msg}");
            assert!(
                v.reason.contains(hint) || v.solution.contains(hint),
                "{msg} -> 原因 {} / 建议 {}",
                v.reason,
                v.solution
            );
            assert!(v.technical.contains(msg), "技术信息须保留原文：{msg}");
        }
    }

    #[test]
    fn 未知原因统一兜底且保留原始技术信息() {
        let a = pv(PrintStage::Print, "驱动返回了未预期的结果 A");
        let b = pv(PrintStage::Print, "驱动返回了未预期的结果 B");
        assert_eq!(a.title, "打印失败");
        assert_eq!(a.kind, "原因未获取");
        assert_eq!(a.reason, b.reason, "兜底原因必须完全一致");
        assert_eq!(a.solution, b.solution, "兜底建议必须完全一致");
        assert_ne!(a.technical, b.technical, "原始错误仍要各自保留");
        assert!(a.technical.contains("驱动返回了未预期的结果 A"));
    }

    #[test]
    fn 取数与预览失败给出对应来源的文案() {
        let v = classify_print_failure(
            PrintStage::Prepare,
            &PrintError::Voided("PM-2026-001".into()),
        );
        assert_eq!(v.kind, "打印数据准备失败");
        assert!(v.reason.contains("已作废"));
        assert!(v.reason.contains("未向打印机发送任何内容"));
        assert!(v.solution.contains("刷新页面"));

        let p = pv(PrintStage::Preview, "创建预览画布失败");
        assert_eq!(p.kind, "预览渲染失败");
        assert!(p.reason.contains("出纸链路不受影响"));
        assert!(p.solution.contains("直接点「打印」"));
    }

    #[test]
    fn 成功与取消都不生成失败弹窗() {
        let cancelled: std::result::Result<usize, PrintError> = Ok(0);
        let success: std::result::Result<usize, PrintError> = Ok(4);
        let failed: std::result::Result<usize, PrintError> =
            Err(PrintError::Print("第 1 页输出失败".into()));
        assert!(failure_for_outcome(PrintStage::Print, &cancelled).is_none());
        assert!(failure_for_outcome(PrintStage::Print, &success).is_none());
        let v = failure_for_outcome(PrintStage::Print, &failed).expect("失败必须生成弹窗");
        assert_eq!(v.title, "打印失败");
    }

    #[test]
    fn 失败弹窗同时只存在一个且关闭后可再次弹出() {
        let mut slot = None;
        let first = pv(PrintStage::Print, "无法开始打印作业（检查打印机是否就绪）");
        let second = pv(PrintStage::Print, "打印作业结束失败");
        assert!(offer_print_failure(&mut slot, first.clone()));
        assert!(
            !offer_print_failure(&mut slot, second.clone()),
            "未关闭时必须忽略后续失败"
        );
        assert_eq!(slot.as_ref(), Some(&first), "必须保留首次原因");
        dismiss_print_failure(&mut slot);
        assert!(slot.is_none(), "关闭只清提示，预览单据与页码由调用方保留");
        assert!(offer_print_failure(&mut slot, second), "关闭后可再次弹出");
        assert_eq!(slot.as_ref().map(|v| v.kind), Some("作业未正常结束"));
    }

    // ------------------------------------------------------------ R-44 打印成功提示

    #[test]
    fn 成功视图包含本次打印信息且标题固定() {
        let v = PrintSuccessView::new("拨款单 PM-2026-001 等 3 张".into(), 3, 6);
        assert_eq!(v.title, "打印已提交");
        assert!(v.doc_name.contains("PM-2026-001"));
        assert_eq!(v.sheets, 3);
        assert_eq!(v.pages, 6);
        assert_eq!(v.submitted_at.len(), 19, "提交时间须为本地时间格式");
        assert!(!print_success_expired(&v), "刚创建不能立即过期");
        assert_eq!(print_success_remaining_secs(&v), PRINT_SUCCESS_SECS);
    }

    #[test]
    fn 成功提示五秒后自动关闭且倒计时不为零() {
        let mut v = PrintSuccessView::new("拨款单 PM-2026-001".into(), 1, 2);
        v.at = Instant::now() - Duration::from_millis(4_900);
        assert!(!print_success_expired(&v), "4.9 秒时仍在显示");
        assert_eq!(print_success_remaining_secs(&v), 1);
        v.at = Instant::now() - Duration::from_secs(6);
        assert!(print_success_expired(&v), "满 5 秒必须自动关闭");
        assert_eq!(
            print_success_remaining_secs(&v),
            1,
            "即使已过期，倒计时文案也不得显示 0 秒"
        );
    }

    #[test]
    fn 成功提示同时只存在一个且关闭后可再弹() {
        let mut slot = None;
        let first = PrintSuccessView::new("拨款单 A".into(), 1, 2);
        let second = PrintSuccessView::new("拨款单 B".into(), 2, 4);
        assert!(offer_print_success(&mut slot, first.clone()));
        assert!(
            !offer_print_success(&mut slot, second.clone()),
            "未关闭时必须忽略后续成功，避免覆盖或重复弹出"
        );
        assert_eq!(slot.as_ref(), Some(&first), "必须保留首次打印信息");
        dismiss_print_success(&mut slot);
        assert!(slot.is_none());
        assert!(offer_print_success(&mut slot, second), "关闭后可再次弹出");
    }

    #[test]
    fn 失败优先清理成功提示且两者不叠加() {
        let mut failure = None;
        let mut success = Some(PrintSuccessView::new("拨款单 A".into(), 1, 2));
        let first = pv(PrintStage::Print, "第 1 页输出失败");
        assert!(offer_print_failure_exclusive(
            &mut failure,
            &mut success,
            first.clone()
        ));
        assert!(success.is_none(), "显示失败时必须清掉成功提示");
        assert_eq!(failure.as_ref(), Some(&first));

        // 失败未关闭期间再来一次失败：成功提示仍要清掉，失败保留首次原因
        let mut success2 = Some(PrintSuccessView::new("拨款单 B".into(), 1, 2));
        let later = pv(PrintStage::Print, "打印作业结束失败");
        assert!(!offer_print_failure_exclusive(
            &mut failure,
            &mut success2,
            later
        ));
        assert!(success2.is_none());
        assert_eq!(failure.as_ref().map(|v| v.kind), Some("出纸中断"));
    }

    #[test]
    fn 任一打印提示在场都禁止再次发起打印() {
        assert!(!print_busy(&None, &None));
        assert!(print_busy(
            &Some(pv(PrintStage::Print, "第 1 页输出失败")),
            &None
        ));
        assert!(print_busy(
            &None,
            &Some(PrintSuccessView::new("拨款单 A".into(), 1, 2))
        ));
    }

    #[test]
    fn 取消与失败都不生成成功提示且只有成功页数才提示() {
        let cancelled: std::result::Result<usize, PrintError> = Ok(0);
        let failed: std::result::Result<usize, PrintError> =
            Err(PrintError::Print("第 1 页输出失败".into()));
        let success: std::result::Result<usize, PrintError> = Ok(4);
        assert!(success_for_outcome(&cancelled, "拨款单 A".into(), 1).is_none());
        assert!(success_for_outcome(&failed, "拨款单 A".into(), 1).is_none());
        let v = success_for_outcome(&success, "拨款单 A 等 2 张".into(), 2)
            .expect("成功必须生成提示弹窗");
        assert_eq!(v.title, "打印已提交");
        assert_eq!(v.sheets, 2);
        assert_eq!(v.pages, 4);
    }

    // ------------------------------------------------------------ R-46 导出列选择

    #[test]
    fn 导出列选择默认全选且表头文字一致() {
        let v = ExportColumnsView::new(ReportResult::period(&[]));
        assert_eq!(v.choices.len(), 4);
        assert!(v.choices.iter().all(|c| c.selected));
        assert_eq!(v.choices[2].label, "支付金额（元）");
        assert_eq!(
            v.selected_keys(),
            vec!["period_month", "payment_count", "amount", "share"]
        );
        assert_eq!(v.confirm().unwrap().columns.len(), 4);
    }

    #[test]
    fn 导出列选择可调序且确认结果顺序与勾选一致() {
        let mut v = ExportColumnsView::new(ReportResult::period(&[]));
        v.set_all(false);
        v.choices[0].selected = true; // 期间
        v.choices[2].selected = true; // 支付金额
        assert_eq!(v.selected_keys(), vec!["period_month", "amount"]);
        // 把「期间」一路挪到「支付金额」之后：调序按列表位置生效
        v.move_down(0);
        v.move_down(1);
        assert_eq!(v.selected_keys(), vec!["amount", "period_month"]);
        let picked = v.confirm().unwrap();
        assert_eq!(
            picked.columns.iter().map(|c| c.title).collect::<Vec<_>>(),
            vec!["支付金额", "期间"]
        );
        assert_eq!(picked.columns.len(), 2);
        picked.validate_shape().unwrap();
    }

    #[test]
    fn 导出列选择空选择被挡住且恢复默认回到原始列序() {
        let mut v = ExportColumnsView::new(ReportResult::period(&[]));
        v.set_all(false);
        assert_eq!(v.confirm(), Err(ReportColumnError::EmptySelection));
        v.move_down(0);
        v.reset();
        assert!(v.choices.iter().all(|c| c.selected));
        assert_eq!(
            v.selected_keys(),
            vec!["period_month", "payment_count", "amount", "share"]
        );
    }

    #[test]
    fn 关闭导出列选择后弹窗与数据都被清除() {
        let mut dialog = Dialog::ExportColumns;
        let mut view = Some(ExportColumnsView::new(ReportResult::period(&[])));
        dismiss_export_columns(&mut dialog, &mut view);
        assert!(dialog == Dialog::None);
        assert!(view.is_none());
    }

    // ------------------------------------------------ 计划交工自动计算（R-50）

    #[test]
    fn 计划交工按含开工当日自动填充() {
        let mut end = String::new();
        let mut auto = None;
        assert!(auto_planned_end("2026-03-01", "90", &mut end, &mut auto));
        assert_eq!(end, "2026-05-29", "90 日历天含开工当日 → 开工 + 89 天");
        assert!(!planned_end_is_manual(&end, &auto), "自动值不算手改");

        // 仍在自动状态时，改工期应跟着更新
        assert!(auto_planned_end("2026-03-01", "91", &mut end, &mut auto));
        assert_eq!(end, "2026-05-30");
        // 改开工日同样跟随
        assert!(auto_planned_end("2026-04-01", "91", &mut end, &mut auto));
        assert_eq!(end, "2026-06-30");
    }

    #[test]
    fn 工期或开工日不完整时不触发计算() {
        // 任一不同时成立：开工日为空/非法、工期为空/0/负数/非数字
        for (start, days) in [
            ("", "90"),
            ("2026-03-01", ""),
            ("2026-03-01", "0"),
            ("2026-03-01", "-5"),
            ("2026-03-01", "九十万"),
            ("2026-02-30", "90"),
        ] {
            let mut end = "2026-06-01".to_string();
            let mut auto = None;
            assert!(
                !auto_planned_end(start, days, &mut end, &mut auto),
                "[{start} / {days}] 不应触发计算"
            );
            assert_eq!(end, "2026-06-01", "不触发时原值必须保持不变");
            assert!(auto.is_none());
        }
    }

    #[test]
    fn 手改计划交工后停止同步且清空可恢复() {
        let mut end = String::new();
        let mut auto = None;
        auto_planned_end("2026-03-01", "90", &mut end, &mut auto);
        assert_eq!(end, "2026-05-29");

        // 用户手改 → 标记为手改，后续改工期不再覆盖
        end = "2026-06-30".to_string();
        assert!(planned_end_is_manual(&end, &auto));
        assert!(!auto_planned_end("2026-03-01", "120", &mut end, &mut auto));
        assert_eq!(end, "2026-06-30", "手改值必须被保留");

        // 清空 → 回到未填状态，恢复自动
        end.clear();
        assert!(!planned_end_is_manual(&end, &auto), "空值不算手改");
        assert!(auto_planned_end("2026-03-01", "120", &mut end, &mut auto));
        assert_eq!(end, "2026-06-28");
    }

    #[test]
    fn 质保金率文本与万分比互转() {
        assert_eq!(bp_text(300), "3");
        assert_eq!(bp_text(350), "3.5");
        assert_eq!(bp_text(0), "0");
        assert_eq!(pct_to_bp(&bp_text(300)), 300);
        assert_eq!(pct_to_bp(&bp_text(125)), 125);
    }

    fn proj(id: &str, code: &str, name: &str) -> ctp_core::project::Project {
        ctp_core::project::Project {
            id: id.into(),
            project_code: code.into(),
            project_name: name.into(),
            ..Default::default()
        }
    }

    /// R-51：修改窗里所属项目只显示名称，不带编码（与新建时录入的一致）。
    #[test]
    fn 所属项目预填只写名称() {
        let c = ctp_core::contract::Contract {
            project_id: "PJ-1".into(),
            ..Default::default()
        };
        let projects = vec![proj("PJ-1", "XAJT", "测试项目")];
        let f = EditContractForm::from_contract(&c, &projects, &[]);
        assert_eq!(f.project_text, "测试项目", "不能把编码也塞进输入框");
    }

    /// R-51：候选列表仍按「编码 名称」展示（方便用编码检索），
    /// 但点选后只回填名称，避免编码留在框里被误认为「已选定」。
    #[test]
    fn 点选项目候选只回填名称() {
        let src = project_sources(&[
            proj("PJ-1", "XAJT", "测试项目"),
            proj("PJ-2", "PRJ-2026-002", "另一个项目"),
        ]);
        let labels: Vec<&str> = src.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["XAJT 测试项目", "PRJ-2026-002 另一个项目"]);
        assert_eq!(pick_project_name(&src, "XAJT 测试项目"), "测试项目");
        assert_eq!(
            pick_project_name(&src, "PRJ-2026-002 另一个项目"),
            "另一个项目"
        );
        // 候选取不到时保留用户原文（允许直接录一个新项目）
        assert_eq!(pick_project_name(&src, "全新项目"), "全新项目");
    }
}
