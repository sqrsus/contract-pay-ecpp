// release 构建声明为 Windows GUI 子系统：双击不再附带黑色控制台窗口，
// 也就不会出现「关掉控制台把程序一起关掉」的情况。debug 构建保留控制台便于看日志。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! 工程合同台账与拨款管理平台 —— 桌面界面入口。
//!
//! 设计目标：把命令行里的「台账 / 拨款 / 汇总 / 自检」搬到可点选、可缩放的窗口里，
//! 同时保持数据口径与 CLI 完全一致（同一套 `ctp-store` / `ctp-app`，同一个 SQLite 库）。

mod dialogs;
mod fonts;
mod screens;
mod theme;
mod widgets;

use ctp_app::checks::CheckReport;
use ctp_app::report::ReportKind;
use ctp_core::attachment::{Attachment, AttachmentKind};
use ctp_core::contract::{Contract, ContractStatus};
use ctp_core::ledger::{CategoryRow, ContractSummary, PeriodRow};
use ctp_core::payment::{Payment, PaymentStatus};
use ctp_core::project::{Contractor, Project};
use ctp_store::{db, repo};
use eframe::egui::{self, Color32, Margin, RichText, Rounding};
use raw_window_handle::HasWindowHandle;
use rusqlite::Connection;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use dialogs::{
    classify_print_failure, dismiss_print_success, failure_for_outcome,
    offer_print_failure_exclusive, offer_print_success, print_busy, print_success_expired,
    success_for_outcome, AmountForm, AuditForm, ContractForm, Dialog, PayForm, PrintStage,
};
use theme::{Palette, ThemeMode};
use widgets::Tone;

/// 导航页。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Contracts,
    Payments,
    /// 汇总统计：合并了原「分类汇总」与「期间统计」两个页面
    Summary,
    /// 操作日志：所有写库操作的留痕与清理（仅管理员可见）
    Logs,
    Checks,
}

impl Screen {
    pub const ALL: [Screen; 6] = [
        Screen::Dashboard,
        Screen::Contracts,
        Screen::Payments,
        Screen::Summary,
        Screen::Logs,
        Screen::Checks,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Screen::Dashboard => "总览",
            Screen::Contracts => "合同台账",
            Screen::Payments => "拨款管理",
            Screen::Summary => "汇总统计",
            Screen::Logs => "操作日志",
            Screen::Checks => "自检与设置",
        }
    }

    /// 窄屏折叠时用的两字缩写（单字辨识度太低）。
    pub fn short(self) -> &'static str {
        match self {
            Screen::Dashboard => "总览",
            Screen::Contracts => "合同",
            Screen::Payments => "拨款",
            Screen::Summary => "汇总",
            Screen::Logs => "日志",
            Screen::Checks => "自检",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Screen::Dashboard => "关键指标与最近动态",
            Screen::Contracts => "合同清单、审计价与金额维护",
            Screen::Payments => "登记拨款单并实时校验可付额度",
            Screen::Summary => "按日 / 项目 / 承包单位统计已付拨款",
            Screen::Logs => "写库操作留痕、按时间段或全部清理",
            Screen::Checks => "口径自检、管理密码",
        }
    }
}

/// 顶部/底部提示条。
pub struct Toast {
    pub text: String,
    pub tone: Tone,
    pub at: Instant,
}

/// 待确认的破坏性操作。
pub enum Confirm {
    // 两个业务实体都较大，装箱让枚举本身保持指针大小。
    VoidPayment(Box<Payment>),
    VoidContract(Box<Contract>),
}

/// 汇总统计的筛选条件快照（与当前筛选值不同就重算）。
pub type SummaryKey = (
    ctp_core::ledger::SummaryDim,
    String,
    String,
    Option<bool>,
    Option<String>,
    String,
);

/// 冒烟/截图任务：`CTP_SELFTEST=1` 跑满帧退出；`CTP_SHOT=路径` 保存一帧界面。
pub struct ShotJob {
    pub path: String,
    /// 还要等多少帧再请求截图（等字体与数据就绪）
    pub wait: u32,
    pub requested: bool,
}

/// 批量导入的来源。
///
/// 两条链路共用同一套护栏（隐藏校验表、文件/行数上限、逐行事务、结果弹窗），
/// 差别只在「读进哪个模块」与文案，所以用同一个待办槽位 + 一个来源标记，
/// 避免为合同再造一份 `pending_*` 状态与一套弹窗。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchImport {
    /// 批量拨款（`ctp_app::batch`）
    Payments,
    /// 批量合同（`ctp_app::contract_batch`）
    Contracts,
}

impl BatchImport {
    /// 弹窗标题与提示里用的主体名。
    pub fn subject(self) -> &'static str {
        match self {
            BatchImport::Payments => "批量拨款",
            BatchImport::Contracts => "批量合同",
        }
    }

    /// 「处理中」遮罩上的文案。
    pub fn busy_text(self) -> &'static str {
        match self {
            BatchImport::Payments => "正在导入批量拨款…",
            BatchImport::Contracts => "正在导入批量合同…",
        }
    }
}

/// 应用状态。
pub struct App {
    pub mode: ThemeMode,
    pub pal: Palette,
    /// 需要重新应用全局样式（首帧与切换主题时置位）
    pub style_dirty: bool,
    pub screen: Screen,
    pub narrow: bool,

    pub db: Option<Connection>,
    pub db_path: String,
    pub init_error: Option<String>,
    /// R-39：只读查询失败时保留的一条可见警告；内容不变就不重复刷提示条。
    /// 顶栏上的「重试读取」会清掉它并重新载入全部视图数据。
    pub data_warning: Option<String>,
    /// R-39：耗时操作的「处理中」状态。批量导入仍是同步的（本轮不做线程化），
    /// 该状态用于挡住重复提交，并在下一帧真正执行前给出可见反馈。
    pub busy: Option<String>,
    /// 上一帧登记的批量导入：（来源, 文件）。来源决定调哪个模块的 `import`。
    pub pending_import: Option<(BatchImport, std::path::PathBuf)>,

    pub summaries: Vec<ContractSummary>,
    pub projects: Vec<Project>,
    pub contractors: Vec<Contractor>,
    /// 历史合同里出现过的甲方名称（甲方联想输入的兜底候选）
    pub party_a_names: Vec<String>,
    /// 当前查看的合同附件（`Dialog::Attachments`）
    pub attachments: Vec<Attachment>,
    /// 附件弹窗对应的合同
    pub att_contract: Option<String>,
    /// 当前选中合同的附件数量（详情卡「附件 n/4」）
    pub att_count: i64,
    pub payments: Vec<Payment>,
    pub categories: Vec<CategoryRow>,
    pub periods: Vec<PeriodRow>,
    pub report: Option<CheckReport>,
    pub schema_version: String,

    pub search: String,
    pub status_filter: Option<ContractStatus>,
    pub selected: Option<String>,
    pub detail: Option<Contract>,
    /// 合同台账里展开拨款明细的合同（点击合同名称切换）
    pub expanded_contract: Option<String>,

    pub pay_contract: Option<String>,
    /// 拨款管理页合同下拉的筛选词。它只是过滤条件，选中哪份合同以 `pay_contract` 为准（R-53）
    pub pay_filter: String,
    /// 合同下拉是否展开（截图与回归核对时可直接置为展开）
    pub pay_open: bool,
    pub pay_form: PayForm,
    /// 正在查看的拨款单（`Dialog::PaymentDetail`）。
    pub pay_detail: Option<Payment>,
    /// 本次要打印的拨款单版面数据（单张 = 1 条；批量 = 勾选的若干条，按列表顺序）
    pub print_vouchers: Vec<ctp_app::Voucher>,
    /// 本次要输出的拨款单 id（与 `print_vouchers` 同序，用于提示）
    pub print_ids: Vec<String>,
    /// 打印预览当前页（0 .. 2N），一页对应单据的一页
    pub print_page: usize,
    /// 预览位图纹理及其对应页码，避免每帧重绘
    pub print_tex: Option<(egui::TextureHandle, usize)>,
    /// R-42：打印失败弹窗的数据。它是不改变 `dialog` 的叠加层，
    /// 关闭后预览窗、单据、页码、位图全部保留。
    pub print_failure: Option<crate::dialogs::PrintFailureView>,
    /// R-44：打印成功提示的数据。作业成功提交后显示、5 秒自动关闭，
    /// 是与失败弹窗互斥的另一个叠加层，同样不改变 `dialog`。
    pub print_success: Option<crate::dialogs::PrintSuccessView>,
    /// R-42：当前页预览渲染已失败，不再每帧重试，避免重复报错 / 重复弹窗。
    /// 翻页或重新打开预览时清零。
    pub print_render_failed: bool,
    /// 主窗口句柄（打印对话框的 owner），未取到时为 0
    pub hwnd: isize,
    /// 拨款管理页「批量打印」的勾选集合
    pub print_sel: std::collections::BTreeSet<String>,
    /// 新建合同表单里的一次性提示（附件选择失败等）
    pub new_contract_flash: Option<String>,
    pub new_contract: ContractForm,
    /// 合同详情只读弹窗（点击合同编号）
    pub modal_contract: Option<Contract>,
    pub audit_form: AuditForm,
    /// 项目交工弹窗的日期输入
    pub delivery_form: String,
    pub amount_form: AmountForm,

    // ---------------- 登录与权限（角色只在进程内有效，关掉程序即失效）
    /// 当前登录身份；`None` 表示停在登录页
    pub role: Option<ctp_app::auth::Role>,
    /// 登录页的密码输入框
    pub login_pw: String,
    /// 登录页的即时错误提示
    pub login_err: Option<String>,
    /// 「管理登录」是否已展开密码框
    pub login_admin: bool,
    /// 登录审计留痕失败时的可见警告 + 待重试的登录结果（R-39）。
    /// 审计失败不阻止登录，但必须让人看得见并能重试，而不是静默 `let _ =`。
    pub login_audit_warn: Option<(bool, String)>,
    /// 设置页修改密码表单：旧密码 / 新密码 / 确认
    pub pw_old: String,
    pub pw_new: String,
    pub pw_new2: String,

    // ---------------- 汇总统计
    /// 当前维度（按项目 / 按承包单位 / 按合同 / 按合同类型）
    pub summary_dim: ctp_core::ledger::SummaryDim,
    /// 统计区间（本地日期闭区间，空串 = 不限）
    pub summary_from: String,
    pub summary_to: String,
    /// 筛选：项目状态（None = 全部；true = 归档关闭，false = 履行中）
    pub summary_archived: Option<bool>,
    /// 筛选：合同类型代码（None = 全部）
    pub summary_type: Option<String>,
    /// 筛选：关键词（项目名 / 承包单位 / 合同编号 / 合同名称）
    pub summary_kw: String,
    /// 当前结果（屏幕与导出同源）
    pub summary: Option<ctp_app::report::ReportResult>,
    /// 上次查询用的筛选快照；与当前值不同就重算
    pub summary_key: Option<SummaryKey>,

    // ---------------- 操作日志（仅管理员可见）
    /// 筛选：起 / 止（本地日期闭区间，空串 = 不限）
    pub log_from: String,
    pub log_to: String,
    /// 筛选：操作类型（None = 全部）
    pub log_action: Option<String>,
    /// 筛选：关键词（匹配对象名称 / 内容 / 目标 id）
    pub log_kw: String,
    /// 当前页数据与计数
    pub log_rows: Vec<ctp_store::audit::AuditLogRow>,
    /// 当前筛选命中的条数
    pub log_count: i64,
    /// 全库条数（显示容量占用）
    pub log_total: i64,
    /// 出现过的操作类型（筛选项，动态取自库）
    pub log_actions: Vec<String>,
    /// 上次查询用的筛选快照；与当前值不同就重查
    pub log_key: Option<(String, String, Option<String>, String)>,
    /// 待确认的清理动作（筛选条件 + 一句人话描述）
    pub log_clear: Option<(ctp_store::audit::LogFilter, String)>,

    pub dialog: Dialog,
    /// 批量导入结果弹窗的数据（与 `Dialog::ImportResult` 配套，保持 Dialog 可 Copy）
    pub import_result: Option<crate::dialogs::ImportResultView>,
    /// 合同台账导出列选择弹窗的数据（R-46，与 `Dialog::ExportColumns` 配套）
    pub export_columns: Option<crate::dialogs::ExportColumnsView>,
    /// 合同信息修改弹窗的数据（R-50，与 `Dialog::EditContract` 配套）
    pub edit_contract: Option<crate::dialogs::EditContractForm>,
    /// 修改成功后的「本次修改」弹窗（R-51，叠加层，不占 `Dialog`）
    pub edit_done: Option<crate::dialogs::EditDoneView>,
    pub confirm: Option<Confirm>,
    pub confirm_reason: String,
    pub toast: Option<Toast>,
    /// 冒烟模式剩余帧数（`CTP_SELFTEST=1` 时启用）
    pub smoke: Option<u32>,
    /// 自动截图任务（`CTP_SHOT=<文件>` 时启用）
    pub shot: Option<ShotJob>,
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// GUI 子系统没有控制台，崩溃信息默认无处可看：把 panic 落到 exe 同级 `data/panic.log`。
fn install_panic_log() {
    let dir = exe_dir().join("data");
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::fs::create_dir_all(&dir);
        let body = format!("[{}] {info}\n", ctp_core::date::now_iso());
        let _ = std::fs::write(dir.join("panic.log"), body);
    }));
}

/// 启动主题，可用环境变量 `CTP_THEME=dark` 指定（默认浅色）。
fn initial_theme() -> ThemeMode {
    match std::env::var("CTP_THEME").unwrap_or_default().as_str() {
        "dark" | "深色" => ThemeMode::Dark,
        _ => ThemeMode::Light,
    }
}

/// 启动页，可用环境变量 `CTP_SCREEN` 指定（默认总览）。
fn initial_screen() -> Screen {
    match std::env::var("CTP_SCREEN").unwrap_or_default().as_str() {
        "contracts" => Screen::Contracts,
        "payments" => Screen::Payments,
        // 旧的 reports / periods 两个入口统一落到「汇总统计」，避免脚本调用失效
        "reports" | "summary" | "periods" => Screen::Summary,
        "logs" => Screen::Logs,
        "checks" => Screen::Checks,
        _ => Screen::Dashboard,
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let _font = fonts::install(&cc.egui_ctx);
        let mode = initial_theme();
        theme::apply(&cc.egui_ctx, mode);
        let pal = Palette::of(mode);

        let dir = exe_dir().join("data");
        let path = dir.join("ctp.db");
        let mut app = App {
            mode,
            pal,
            style_dirty: true,
            screen: initial_screen(),
            narrow: false,
            db: None,
            db_path: path.to_string_lossy().to_string(),
            init_error: None,
            data_warning: None,
            busy: None,
            pending_import: None,
            summaries: Vec::new(),
            projects: Vec::new(),
            contractors: Vec::new(),
            party_a_names: Vec::new(),
            attachments: Vec::new(),
            att_contract: None,
            att_count: 0,
            payments: Vec::new(),
            categories: Vec::new(),
            periods: Vec::new(),
            report: None,
            schema_version: String::new(),
            search: String::new(),
            status_filter: None,
            selected: None,
            detail: None,
            expanded_contract: None,
            pay_contract: None,
            pay_filter: String::new(),
            pay_open: false,
            pay_form: PayForm::default(),
            pay_detail: None,
            print_vouchers: Vec::new(),
            print_ids: Vec::new(),
            print_page: 0,
            print_tex: None,
            print_failure: None,
            print_success: None,
            print_render_failed: false,
            hwnd: 0,
            print_sel: std::collections::BTreeSet::new(),
            new_contract: ContractForm::default(),
            new_contract_flash: None,
            modal_contract: None,
            audit_form: AuditForm::default(),
            delivery_form: String::new(),
            amount_form: AmountForm::default(),
            role: None,
            login_pw: String::new(),
            login_err: None,
            login_admin: false,
            login_audit_warn: None,
            pw_old: String::new(),
            pw_new: String::new(),
            pw_new2: String::new(),
            summary_dim: ctp_core::ledger::SummaryDim::Project,
            summary_from: ctp_app::service::default_summary_range().0,
            summary_to: ctp_app::service::default_summary_range().1,
            summary_archived: None,
            summary_type: None,
            summary_kw: String::new(),
            summary: None,
            summary_key: None,
            log_from: String::new(),
            log_to: String::new(),
            log_action: None,
            log_kw: String::new(),
            log_rows: Vec::new(),
            log_count: 0,
            log_total: 0,
            log_actions: Vec::new(),
            log_key: None,
            log_clear: None,
            dialog: Dialog::None,
            import_result: None,
            export_columns: None,
            edit_contract: None,
            edit_done: None,
            confirm: None,
            confirm_reason: String::new(),
            toast: None,
            smoke: if std::env::var("CTP_SELFTEST").as_deref() == Ok("1") {
                Some(40)
            } else {
                None
            },
            shot: std::env::var("CTP_SHOT").ok().map(|path| ShotJob {
                path,
                wait: std::env::var("CTP_SHOT_FRAME")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(40),
                requested: false,
            }),
        };

        if let Err(e) = std::fs::create_dir_all(&dir) {
            app.init_error = Some(format!("创建数据目录失败：{e}"));
            return app;
        }
        match db::open(path.to_string_lossy().as_ref()) {
            Ok(mut conn) => {
                if let Err(e) = db::migrate(&mut conn) {
                    app.init_error = Some(format!("初始化数据库结构失败：{e}"));
                    return app;
                }
                if let Err(e) = ctp_store::seed::seed_reference(&conn, "system") {
                    app.init_error = Some(format!("写入基础字典失败：{e}"));
                    return app;
                }
                // 首次打开写入初始管理密码（0001）；已有密码时原样保留，不动任何业务数据
                if let Err(e) = ctp_app::auth::ensure_initial(&conn) {
                    app.init_error = Some(format!("初始化管理密码失败：{e}"));
                    return app;
                }
                app.db = Some(conn);
                app.refresh();
            }
            Err(e) => app.init_error = Some(format!("打开数据库失败：{e}")),
        }
        app.open_initial_dialog();
        app
    }

    /// 启动时直接打开某个弹窗（`CTP_DIALOG=new_contract|audit_price|edit_amount|payment_detail|print_preview|attachments`）。
    ///
    /// 供自动化截图与问题复现使用；正常启动不受影响。
    fn open_initial_dialog(&mut self) {
        // `CTP_LOGIN=admin` 让登录页直接展开密码框（截图核对用）
        if std::env::var("CTP_LOGIN").as_deref() == Ok("admin") {
            self.login_admin = true;
        }
        // `CTP_ROLE=browse|admin` 跳过登录直达主界面（截图与回归核对用，正常启动不设置）
        match std::env::var("CTP_ROLE").unwrap_or_default().as_str() {
            "browse" => self.role = Some(ctp_app::auth::Role::Browse),
            "admin" => self.role = Some(ctp_app::auth::Role::Admin),
            _ => {}
        }
        // `CTP_DEMO=1`：空库时灌演示数据，供截图与冒烟核对（只在显式开启时生效）
        if std::env::var("CTP_DEMO").unwrap_or_default() == "1" && self.summaries.is_empty() {
            let op = self.operator();
            match self.exec(|c| ctp_app::demo::seed_demo(c, op)) {
                Ok(_) => self.refresh(),
                Err(e) => self.error(format!("载入演示数据失败：{e}")),
            }
        }
        // `CTP_SELECT=1` 仅选中第一份合同，用于核对选中态布局
        if std::env::var("CTP_SELECT").unwrap_or_default() == "1" {
            if let Some(id) = self.summaries.first().map(|s| s.contract_id.clone()) {
                self.select(&id);
                self.screen = Screen::Contracts;
            }
        }
        // `CTP_EXPAND=1` 选中并展开第一份合同的拨款明细，用于核对表头与作废筛除
        if std::env::var("CTP_EXPAND").unwrap_or_default() == "1" {
            if let Some(id) = self.summaries.first().map(|s| s.contract_id.clone()) {
                self.select(&id);
                self.expanded_contract = Some(id);
                self.screen = Screen::Contracts;
            }
        }
        // `CTP_PAY_NO=HT-...` 直达指定合同的拨款页，用于核对限额与状态展示
        if let Ok(no) = std::env::var("CTP_PAY_NO") {
            if let Some(s) = self.summaries.iter().find(|s| s.contract_no == no).cloned() {
                self.pay_contract = Some(s.contract_id.clone());
                self.screen = Screen::Payments;
            }
        }
        // `CTP_PAY_FILTER=...` 预填筛选词并展开合同下拉，用于核对筛选结果与列表（R-53）
        if let Ok(t) = std::env::var("CTP_PAY_FILTER") {
            if self.pay_contract.is_none() {
                self.pay_contract = self.summaries.first().map(|s| s.contract_id.clone());
            }
            self.pay_filter = t;
            self.pay_open = true;
            self.screen = Screen::Payments;
        }
        match std::env::var("CTP_DIALOG").unwrap_or_default().as_str() {
            "new_contract" => {
                self.dialog = Dialog::NewContract;
                // `CTP_STEP=1` 直达第二步（附件），供截图核对
                if std::env::var("CTP_STEP").unwrap_or_default() == "1" {
                    self.new_contract.step = 1;
                }
            }
            "attachments" => {
                if let Some(id) = self.summaries.first().map(|s| s.contract_id.clone()) {
                    self.select(&id);
                    self.screen = Screen::Contracts;
                    self.open_attachments(&id);
                }
            }
            "audit_price" => {
                if let Some(id) = self.summaries.first().map(|s| s.contract_id.clone()) {
                    self.select(&id);
                    self.screen = Screen::Contracts;
                    self.dialog = Dialog::AuditPrice;
                }
            }
            "edit_amount" => {
                if let Some(id) = self.summaries.first().map(|s| s.contract_id.clone()) {
                    self.select(&id);
                    self.screen = Screen::Contracts;
                    self.dialog = Dialog::EditAmount;
                }
            }
            "void_payment" => {
                if let Some(pm) = self.payments.first().cloned() {
                    self.screen = Screen::Payments;
                    self.pay_contract = Some(pm.contract_id.clone());
                    self.confirm = Some(Confirm::VoidPayment(Box::new(pm)));
                }
            }
            "payment_detail" => {
                if let Some(pm) = self.payments.first().cloned() {
                    self.pay_detail = Some(pm);
                    self.dialog = Dialog::PaymentDetail;
                }
            }
            "contract_detail" => {
                if let Some(id) = self.summaries.first().map(|s| s.contract_id.clone()) {
                    self.open_contract_detail(&id);
                }
            }
            // R-51：合同信息修改窗与「本次修改」弹窗，供截图核对
            "edit_contract" => {
                if let Some(id) = self.summaries.first().map(|s| s.contract_id.clone()) {
                    self.select(&id);
                    self.screen = Screen::Contracts;
                    self.open_edit_contract(&id);
                }
            }
            "edit_done" => {
                if let Some(s) = self.summaries.first() {
                    let (no, name) = (s.contract_no.clone(), s.contract_name.clone());
                    self.select(&s.contract_id.clone());
                    self.screen = Screen::Contracts;
                    self.edit_done = Some(crate::dialogs::EditDoneView::sample(&no, &name));
                }
            }
            "material_print" => {
                if let Some(id) = self.summaries.first().map(|s| s.contract_id.clone()) {
                    self.select(&id);
                    self.screen = Screen::Contracts;
                    self.open_material_print(&id);
                }
            }
            "print_preview" => {
                // 作废单不允许打印，这里直接跳过作废的记录
                if let Some(pm) = self
                    .payments
                    .iter()
                    .find(|p| p.status == PaymentStatus::Paid)
                    .cloned()
                {
                    self.pay_contract = Some(pm.contract_id.clone());
                    self.screen = Screen::Payments;
                    self.open_print_preview(&pm);
                }
            }
            "print_batch" => {
                // 勾选同一合同下前两张已付单，用于核对批量打印条与合并张数
                if let Some(pm) = self
                    .payments
                    .iter()
                    .find(|p| p.status == PaymentStatus::Paid)
                    .cloned()
                {
                    self.pay_contract = Some(pm.contract_id.clone());
                    self.screen = Screen::Payments;
                    let ids: Vec<String> = self
                        .payments
                        .iter()
                        .filter(|p| {
                            p.contract_id == pm.contract_id && p.status == PaymentStatus::Paid
                        })
                        .take(2)
                        .map(|p| p.id.clone())
                        .collect();
                    self.print_sel.extend(ids);
                }
            }
            _ => {}
        }
    }

    /// 重新载入全部视图数据。写操作之后必须调用。
    ///
    /// R-39：任何一个查询失败都不再静默保留旧缓存，而是汇总成一条可见警告，
    /// 由顶栏「重试读取」重新执行；`data_warning` 内容不变时不重复刷提示条。
    pub fn refresh(&mut self) {
        let mut errs: Vec<String> = Vec::new();
        {
            let Some(conn) = self.db.as_ref() else {
                return;
            };
            macro_rules! load {
                ($label:expr, $call:expr, $field:ident) => {
                    match $call {
                        Ok(v) => self.$field = v,
                        Err(e) => errs.push(format!("{}({e})", $label)),
                    }
                };
            }
            load!("合同汇总", repo::summaries(conn), summaries);
            load!("项目档案", repo::list_projects(conn), projects);
            load!("单位档案", repo::list_contractors(conn), contractors);
            load!("发包人候选", repo::list_party_a_names(conn), party_a_names);
            load!("拨款明细", repo::list_payments(conn, None), payments);
            load!("分类汇总", repo::category_summary(conn), categories);
            load!("期间统计", repo::period_summary(conn), periods);
            match db::schema_version(conn) {
                Ok(Some(v)) => self.schema_version = v,
                Ok(None) => {}
                Err(e) => errs.push(format!("结构版本({e})")),
            }
            // 选中项跟随数据刷新：合同不在了就清空
            if let Some(id) = self.selected.clone() {
                match repo::get_contract(conn, &id) {
                    Ok(c) => self.detail = Some(c),
                    Err(_) => {
                        self.selected = None;
                        self.detail = None;
                    }
                }
            }
        }
        // 写操作之后日志一定有新条目：置空快照让操作日志页下一帧重查
        self.log_key = None;
        if self.pay_contract.is_none() {
            self.pay_contract = self.summaries.first().map(|s| s.contract_id.clone());
        }
        if errs.is_empty() {
            self.data_warning = None;
        } else {
            self.warn_data("刷新视图数据", errs.join("；"));
        }
    }

    /// 只读查询失败：合并成一条可见警告（toast + 顶栏常驻提示）。
    pub fn warn_data(&mut self, what: &str, err: impl std::fmt::Display) {
        let msg = format!("{what}失败：{err}");
        if self.data_warning.as_deref() != Some(msg.as_str()) {
            self.error(msg.clone());
            self.data_warning = Some(msg);
        }
    }

    /// 顶栏「重试读取」：清掉警告并重新载入全部视图与页面级快照。
    pub fn retry_data(&mut self) {
        self.data_warning = None;
        self.log_key = None;
        self.summary_key = None;
        self.refresh();
        if self.data_warning.is_none() {
            self.success("数据已重新读取");
        }
    }

    /// 选中合同。
    pub fn select(&mut self, id: &str) {
        self.selected = Some(id.to_string());
        self.detail = self
            .db
            .as_ref()
            .and_then(|c| repo::get_contract(c, id).ok());
        self.att_count = self
            .db
            .as_ref()
            .and_then(|c| repo::attachment_count(c, id).ok())
            .unwrap_or(0);
    }

    /// 跳到合同台账并选中该合同（总览里点合同编号时用）。
    pub fn goto_contract(&mut self, id: &str) {
        self.select(id);
        self.screen = Screen::Contracts;
    }

    /// 打开合同详情只读弹窗（点击合同编号）。
    pub fn open_contract_detail(&mut self, id: &str) {
        self.modal_contract = self
            .db
            .as_ref()
            .and_then(|c| repo::get_contract(c, id).ok());
        self.dialog = Dialog::ContractDetail;
    }

    /// 打开「合同信息修改」弹窗（R-50，右侧栏第 5 个按钮）。
    ///
    /// 预填读的是库里**整条**合同，而不是列表摘要：摘要不含 `payment_terms`、
    /// `bid_agent_id` 等字段，用摘要预填会把内容清空。读不到就只提示，不打开空表单。
    pub fn open_edit_contract(&mut self, id: &str) {
        let Some(c) = self
            .db
            .as_ref()
            .and_then(|conn| repo::get_contract(conn, id).ok())
        else {
            self.error("读取合同失败，请刷新后重试");
            return;
        };
        let form =
            crate::dialogs::EditContractForm::from_contract(&c, &self.projects, &self.contractors);
        self.edit_contract = Some(form);
        self.dialog = Dialog::EditContract;
    }

    /// 合同台账点击名称：展开 / 收起该合同的拨款明细。
    pub fn toggle_expand(&mut self, id: &str) {
        self.expanded_contract = if self.expanded_contract.as_deref() == Some(id) {
            None
        } else {
            Some(id.to_string())
        };
    }

    /// 总览「新建拨款」：跳到拨款管理，并尽量定位到已选合同。
    pub fn start_payment(&mut self) {
        if let Some(id) = self.selected.clone() {
            self.pay_contract = Some(id);
        } else if self.pay_contract.is_none() {
            self.pay_contract = self.summaries.first().map(|s| s.contract_id.clone());
        }
        self.screen = Screen::Payments;
    }

    /// 打印附件：加载该合同的附件元数据，打开材料清单（由用户在本机应用里打印）。
    pub fn open_material_print(&mut self, contract_id: &str) {
        self.att_contract = Some(contract_id.to_string());
        self.attachments = self
            .db
            .as_ref()
            .and_then(|c| repo::list_attachments(c, contract_id).ok())
            .unwrap_or_default();
        self.dialog = Dialog::MaterialPrint;
    }

    /// 把某类合同附件实体化到临时目录，用本机应用打开。
    ///
    /// 附件存在库里没有原路径，每次打开都落一份临时副本；打开后由用户决定打哪几页。
    pub fn open_attachment_file(&mut self, kind: AttachmentKind) {
        let Some(cid) = self.att_contract.clone() else {
            return;
        };
        let kind_code = kind.code().to_string();
        let got: Result<Option<(String, Vec<u8>)>, String> = (|| {
            let conn = self.db.as_ref().ok_or_else(|| "数据库未打开".to_string())?;
            let list = repo::list_attachments(conn, &cid).map_err(|e| e.to_string())?;
            let Some(a) = list.into_iter().find(|a| a.kind_code == kind_code) else {
                return Ok(None);
            };
            let bytes =
                ctp_app::service::attachment_bytes(conn, &a.id).map_err(|e| e.to_string())?;
            Ok(Some((a.file_name, bytes)))
        })();
        match got {
            Ok(Some((name, bytes))) => {
                let dir = std::env::temp_dir().join("ctp-attachments");
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(&name);
                match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        if let Err(e) = dialogs::open_external(&path) {
                            self.error(e);
                        } else {
                            self.success(format!(
                                "已用本机应用打开「{}」，在应用内选择打印",
                                kind.label()
                            ));
                        }
                    }
                    Err(e) => self.error(format!("写入临时文件失败：{e}")),
                }
            }
            Ok(None) => self.error(format!("「{}」尚未上传", kind.label())),
            Err(e) => self.error(format!("读取附件失败：{e}")),
        }
    }

    /// 打开附件管理弹窗，并加载该合同的附件清单。
    pub fn open_attachments(&mut self, contract_id: &str) {
        self.att_contract = Some(contract_id.to_string());
        self.attachments = self
            .db
            .as_ref()
            .and_then(|c| repo::list_attachments(c, contract_id).ok())
            .unwrap_or_default();
        self.dialog = Dialog::Attachments;
    }

    /// 替换 / 删除之后刷新附件清单与「n/4」计数。
    pub fn reload_attachments(&mut self) {
        let Some(id) = self.att_contract.clone() else {
            return;
        };
        if let Some(conn) = self.db.as_ref() {
            self.attachments = repo::list_attachments(conn, &id).unwrap_or_default();
            self.att_count = repo::attachment_count(conn, &id).unwrap_or(0);
        }
    }

    /// 打开拨款单详情弹窗（总览、拨款明细里点单号时用）。
    pub fn open_payment(&mut self, id: &str) {
        if let Some(pm) = self.payments.iter().find(|x| x.id == id) {
            self.pay_detail = Some(pm.clone());
            self.dialog = Dialog::PaymentDetail;
        }
    }

    /// 付款打印入口：选中一张拨款单 → 解析版面数据 → 打开预览弹窗。
    pub fn open_print_preview(&mut self, pm: &Payment) {
        self.print_ids = vec![pm.id.clone()];
        self.load_print_vouchers();
    }

    /// 批量打印入口：按调用方给出的 id 列表（已按列表顺序）成页（一张两页）。
    /// 总览「最近拨款」与拨款管理「拨款明细」共用同一套 `print_sel` 与打印逻辑。
    pub fn open_batch_print(&mut self, ids: Vec<String>) {
        if self.print_sel.is_empty() {
            self.error("请先勾选要打印的拨款单");
            return;
        }
        let chosen: Vec<String> = ids
            .into_iter()
            .filter(|id| self.print_sel.contains(id))
            .collect();
        if chosen.is_empty() {
            self.print_sel.clear();
            self.error("勾选的拨款单已不在当前列表中，请重新勾选");
            return;
        }
        self.print_ids = chosen;
        self.load_print_vouchers();
    }

    /// 解析版面数据并打开预览。取数与打印同一入口，不存在第二套口径。
    fn load_print_vouchers(&mut self) {
        // R-42/R-44：任一打印提示未关闭期间忽略新的打开预览请求，避免重复弹出。
        if print_busy(&self.print_failure, &self.print_success) {
            return;
        }
        let ids = self.print_ids.clone();
        let out = self
            .db
            .as_ref()
            .map(|conn| ctp_app::service::print_vouchers(conn, &ids));
        match out {
            Some(Ok(vs)) if !vs.is_empty() => {
                self.print_vouchers = vs;
                self.print_page = 0;
                self.print_tex = None;
                self.print_render_failed = false;
                self.dialog = Dialog::PrintPreview;
            }
            Some(Ok(_)) => self.error("没有可打印的拨款单"),
            Some(Err(e)) => {
                self.show_print_failure(classify_print_failure(PrintStage::Prepare, &e))
            }
            None => self.error("数据库未打开"),
        }
    }

    /// 打印：直接弹系统「打印」对话框并出纸，不经过 Word / PDF。
    ///
    /// R-44：结果三分支互斥且顺序固定——`Err` 走 R-42 统一失败弹窗（展示行为不变）；
    /// `Ok(0)` 用户取消保持静默；`Ok(pages > 0)` 只显示含本次打印信息的成功弹窗，
    /// 5 秒后自动关闭，不再发绿色提示条，避免同一结果出现两处提示。
    pub fn print_now(&mut self) {
        // 任一打印提示未关闭期间忽略重复点击，不再发起第二次出纸。
        if print_busy(&self.print_failure, &self.print_success) {
            return;
        }
        if self.print_vouchers.is_empty() {
            self.error("没有可打印的拨款单");
            return;
        }
        let vs = self.print_vouchers.clone();
        let name = ctp_app::print::doc_name(&vs);
        let outcome = ctp_app::print_vouchers_now(self.hwnd, &vs, &name);
        if let Some(view) = failure_for_outcome(PrintStage::Print, &outcome) {
            self.show_print_failure(view);
        } else if let Some(view) = success_for_outcome(&outcome, name, vs.len()) {
            self.show_print_success(view);
        }
    }

    /// 报表导出：弹保存框 → 写 .xlsx → 落 EXPORT 审计日志。
    ///
    /// 屏幕表格与导出读的是同一份 `ReportResult` 构造逻辑（文档 4.4.2）。
    pub fn export_report(&mut self, kind: ReportKind) {
        // 默认文件名仍由 report_of 给出；查询失败时沿用原来的「{报表名}.xlsx」兜底，
        // 真正的错误交给保存框之后的导出链路提示。
        let file_name = self
            .db
            .as_ref()
            .and_then(|conn| ctp_app::service::report_of(conn, kind).ok())
            .map(|r| r.default_file_name())
            .unwrap_or_else(|| format!("{}.xlsx", kind.file_stem()));
        let op = self.operator();
        self.save_report(kind.title(), &file_name, |conn, path| {
            ctp_app::service::export_report(conn, kind, path, op)
        });
    }

    /// 导出当前汇总结果（区间由界面决定，不能走 `report_of` 的默认区间）。
    pub fn export_summary(&mut self) {
        let Some(result) = self.summary.clone() else {
            self.error("还没有汇总结果，先选好区间");
            return;
        };
        self.export_report_result(&result);
    }

    /// 导出任意一份已构造好的 `ReportResult`（汇总统计 / 合同台账 / 拨款一览通用）。
    /// 屏幕与 Excel 读同一个结果对象，不重新查库、不重算合计。
    pub fn export_report_result(&mut self, result: &ctp_app::report::ReportResult) {
        let file_name = result.default_file_name();
        let op = self.operator();
        self.save_report(result.title, &file_name, |conn, path| {
            ctp_app::service::export_result(conn, result, path, op)
        });
    }

    /// 所有导出入口共用的「保存框 → 写出 → 成功 / 失败提示」。
    ///
    /// 取消保存框用空串静默返回，失败统一走 `导出失败：{e}`，
    /// 成功提示文案保持「已导出「{标题}」{行数} 行 → {路径}」不变。
    fn save_report(
        &mut self,
        title: &str,
        file_name: &str,
        write: impl FnOnce(&Connection, &std::path::Path) -> ctp_store::Result<(usize, usize)>,
    ) {
        let outcome: Result<(usize, PathBuf), String> = (|| {
            let conn = self.db.as_ref().ok_or_else(|| "数据库未打开".to_string())?;
            let path = rfd::FileDialog::new()
                .set_file_name(file_name)
                .add_filter("Excel 工作簿", &["xlsx"])
                .save_file()
                // 用户取消保存框：用空串标记，不弹错误提示
                .ok_or_else(String::new)?;
            let (rows, _) = write(conn, &path).map_err(|e| e.to_string())?;
            Ok((rows, path))
        })();
        match outcome {
            Ok((rows, path)) => {
                self.success(format!("已导出「{title}」{rows} 行 → {}", path.display()))
            }
            Err(e) if e.is_empty() => {}
            Err(e) => self.error(format!("导出失败：{e}")),
        }
    }

    /// 合同台账「导出 Excel」：先把当前筛选 / 排序下的完整结果快照进列选择弹窗，
    /// 用户确认后才投影列并进入保存框，数据范围不会再重算。
    pub fn open_export_columns(&mut self, result: ctp_app::report::ReportResult) {
        self.export_columns = Some(crate::dialogs::ExportColumnsView::new(result));
        self.dialog = Dialog::ExportColumns;
    }

    /// 打开常驻的导入结果弹窗（成功 / 部分失败 / 全部失败 / 整份拒绝都走这里）。
    pub fn show_import_result(&mut self, view: crate::dialogs::ImportResultView) {
        self.import_result = Some(view);
        self.dialog = Dialog::ImportResult;
    }

    /// R-42/R-44：打印失败的唯一入口。展示前先清掉可能还在倒计时的成功提示，
    /// 保证成功与失败互不叠加；失败侧仍只弹统一卡片弹窗、不发红色提示条，
    /// 已有未关闭的失败弹窗时忽略后续失败，保留首次原因，避免重复弹出。
    pub fn show_print_failure(&mut self, view: crate::dialogs::PrintFailureView) {
        let _ =
            offer_print_failure_exclusive(&mut self.print_failure, &mut self.print_success, view);
    }

    /// R-44：打印成功的唯一入口。失败弹窗在场时不展示成功提示；
    /// 已有未关闭的成功提示时忽略后续成功，避免覆盖或重复弹出。
    pub fn show_print_success(&mut self, view: crate::dialogs::PrintSuccessView) {
        if self.print_failure.is_some() {
            return;
        }
        let _ = offer_print_success(&mut self.print_success, view);
    }

    /// 上一帧登记的批量导入，在这一帧真正执行（R-39「处理中」状态的落地）。
    fn run_pending_import(&mut self, kind: BatchImport, path: &std::path::Path) {
        // 两条链路返回同一个 `ImportReport`，结果弹窗与失败行提示完全共用
        let op = self.operator();
        let report = match kind {
            BatchImport::Payments => self.exec(|c| ctp_app::batch::import(c, path, op)),
            BatchImport::Contracts => self.exec(|c| ctp_app::contract_batch::import(c, path, op)),
        };
        let view = match report {
            Ok(report) => {
                self.refresh();
                crate::dialogs::ImportResultView::from_report(&report, kind)
            }
            Err(e) => crate::dialogs::ImportResultView::rejected(kind, format!("导入失败：{e}")),
        };
        self.busy = None;
        self.show_import_result(view);
    }

    pub fn toast(&mut self, tone: Tone, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            tone,
            at: Instant::now(),
        });
    }

    pub fn success(&mut self, text: impl Into<String>) {
        self.toast(Tone::Success, text);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toast(Tone::Danger, text);
    }

    /// 当前身份能否写库（浏览用户一律 false）。
    pub fn can_write(&self) -> bool {
        self.role.map(|r| r.can_write()).unwrap_or(false)
    }

    /// 审计用「操作人」：**唯一来源**，一律取当前登录身份的名称。
    ///
    /// R-57 之前界面各入口各自硬编码（`界面用户` 与 `管理员` 两种口径混杂），
    /// 操作日志里同一个人的记录会显示成两个名字，无法按人筛选核对。
    /// 现在所有写操作都从这里取，日志里的名字与顶栏身份徽标永远一致。
    pub fn operator(&self) -> &'static str {
        self.role.map(|r| r.label()).unwrap_or("未登录")
    }

    /// 当前身份能否查看操作日志。现阶段与写权限一致（管理员）；
    /// 将来加「授权角色」时只需改这一处。
    pub fn can_view_logs(&self) -> bool {
        self.can_write()
    }

    /// 权限不足的统一提示文案。
    pub const NO_PERMISSION: &'static str =
        "无权限：浏览模式仅可查看，请用「管理登录」进入后再操作";

    /// 写操作入口的守卫：浏览用户直接提示「无权限」并返回 false。
    ///
    /// 按钮不置灰（按确认口径：点了才提示），这里把入口挡住，
    /// 避免让浏览用户填完一整张表单才发现存不进去。
    pub fn require_write(&mut self) -> bool {
        if self.can_write() {
            return true;
        }
        self.error(Self::NO_PERMISSION);
        false
    }

    /// 在数据库上执行一次写操作，随后刷新缓存。
    ///
    /// 写入口统一从这里过，所以「浏览用户不得改数据」只需在这一处拦截；
    /// 只读操作（如读取附件字节）走 `db.as_ref()`，不受影响。
    pub fn exec<T>(
        &mut self,
        f: impl FnOnce(&mut Connection) -> Result<T, ctp_store::StoreError>,
    ) -> Result<T, String> {
        if !self.can_write() {
            return Err(Self::NO_PERMISSION.to_string());
        }
        let conn = self.db.as_mut().ok_or_else(|| "数据库未打开".to_string())?;
        let out = f(conn).map_err(|e| e.to_string())?;
        Ok(out)
    }

    /// 只读访问数据库：浏览用户同样可用，不经过写权限检查。
    pub fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, ctp_store::StoreError>,
    ) -> Result<T, String> {
        let conn = self.db.as_ref().ok_or_else(|| "数据库未打开".to_string())?;
        f(conn).map_err(|e| e.to_string())
    }

    pub fn selected_summary(&self) -> Option<&ContractSummary> {
        let id = self.selected.as_ref()?;
        self.summaries.iter().find(|s| &s.contract_id == id)
    }

    pub fn contract_by_id(&self, id: &str) -> Option<&Contract> {
        if self.detail.as_ref().map(|c| c.id.as_str()) == Some(id) {
            return self.detail.as_ref();
        }
        None
    }

    // ---------------------------------------------------------------- 顶栏

    fn top_bar(&mut self, ctx: &egui::Context) {
        let p = self.pal;
        let mut logout = false;
        let mut retry_audit = false;
        let mut retry_data = false;
        egui::TopBottomPanel::top("top_bar")
            .exact_height(62.0)
            .frame(
                egui::Frame::none()
                    .fill(p.surface)
                    .inner_margin(Margin::symmetric(20.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, Rounding::same(9.0), p.primary);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "合",
                        egui::FontId::proportional(17.0),
                        p.on_primary,
                    );
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("工程合同台账与拨款管理平台")
                                .size(16.0)
                                .strong()
                                .color(p.text),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} · {}",
                                self.screen.label(),
                                self.screen.hint()
                            ))
                            .size(11.5)
                            .color(p.text_muted),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // 退出登录：回到登录页，角色清空（不存在任何持久化）
                        if widgets::ghost_button(ui, &p, "退出")
                            .on_hover_text("退出当前身份，回到登录页")
                            .clicked()
                        {
                            logout = true;
                        }
                        ui.add_space(6.0);
                        // R-39：登录留痕写入失败的补写入口，登录后仍可见
                        if self.login_audit_warn.is_some() {
                            if widgets::ghost_button(ui, &p, "重试留痕").clicked() {
                                retry_audit = true;
                            }
                            ui.label(RichText::new("登录留痕待补写").size(11.0).color(p.danger));
                            ui.add_space(6.0);
                        }
                        // R-39：读库失败的可见警告 + 重试入口
                        if let Some(msg) = self.data_warning.clone() {
                            if widgets::ghost_button(ui, &p, "重试读取").clicked() {
                                retry_data = true;
                            }
                            ui.label(RichText::new(msg.clone()).size(11.0).color(p.danger))
                                .on_hover_text(msg);
                            ui.add_space(6.0);
                        }
                        let (role_text, role_tone) = match self.role {
                            Some(r) if r.can_write() => (r.label(), Tone::Primary),
                            Some(r) => (r.label(), Tone::Neutral),
                            None => ("未登录", Tone::Neutral),
                        };
                        widgets::badge(ui, &p, role_text, role_tone);
                        ui.add_space(8.0);
                        let theme_text = if self.mode.is_dark() {
                            "浅色"
                        } else {
                            "深色"
                        };
                        if widgets::ghost_button(ui, &p, &format!("切换{theme_text}")).clicked() {
                            self.mode.toggle();
                            self.pal = Palette::of(self.mode);
                            self.style_dirty = true;
                        }
                        if widgets::ghost_button(ui, &p, "刷新").clicked() {
                            self.refresh();
                            self.success("数据已刷新");
                        }
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(shorten_path(&self.db_path))
                                .size(11.0)
                                .color(p.text_muted),
                        );
                    });
                });
            });
        // 顶栏下沿分隔线
        let rect = egui::Rect::from_min_size(
            egui::pos2(0.0, 61.0),
            egui::vec2(ctx.screen_rect().width(), 1.0),
        );
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("top_bar_line"),
        ))
        .rect_filled(rect, Rounding::same(0.0), p.border);

        if retry_data {
            self.retry_data();
        }
        if retry_audit {
            if let Some((ok, _)) = self.login_audit_warn.clone() {
                let op = self.operator();
                match self
                    .db
                    .as_ref()
                    .map(|c| ctp_app::auth::log_login(c, ok, op))
                {
                    Some(Ok(())) => {
                        self.login_audit_warn = None;
                        self.success("登录留痕已补写");
                    }
                    Some(Err(e)) => self.error(format!("重试留痕失败：{e}")),
                    None => self.error("数据库不可用，无法补写登录留痕"),
                }
            }
        }
        if logout {
            self.role = None;
            crate::dialogs::dismiss_import_result(&mut self.dialog, &mut self.import_result);
            crate::dialogs::dismiss_export_columns(&mut self.dialog, &mut self.export_columns);
            self.edit_contract = None;
            self.edit_done = None;
            crate::dialogs::dismiss_print_failure(&mut self.print_failure);
            dismiss_print_success(&mut self.print_success);
            self.print_render_failed = false;
            self.confirm = None;
            self.modal_contract = None;
            self.screen = Screen::Dashboard;
            self.login_pw.clear();
            self.login_err = None;
            self.login_admin = false;
            self.login_audit_warn = None;
            self.toast(Tone::Neutral, "已退出登录");
        }
    }

    // ---------------------------------------------------------------- 导航

    fn nav(&mut self, ctx: &egui::Context) {
        let p = self.pal;
        let width = if self.narrow { 76.0 } else { 198.0 };
        egui::SidePanel::left("nav")
            .exact_width(width)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(p.surface)
                    .inner_margin(Margin::symmetric(10.0, 14.0)),
            )
            .show(ctx, |ui| {
                if !self.narrow {
                    ui.label(RichText::new("功能导航").size(11.0).color(p.text_muted));
                    ui.add_space(6.0);
                }
                for s in Screen::ALL {
                    // 浏览用户看不到「操作日志」与「自检与设置」：清理与设置都是管理员的事
                    if (s == Screen::Checks || s == Screen::Logs) && !self.can_write() {
                        continue;
                    }
                    let selected = self.screen == s;
                    let text = if self.narrow { s.short() } else { s.label() };
                    let resp = nav_item(ui, &p, text, selected, self.narrow);
                    // 折叠态只显示两字缩写，悬停给出完整名称
                    let resp = if self.narrow {
                        resp.on_hover_text(s.label())
                    } else {
                        resp
                    };
                    if resp.clicked() && !selected {
                        self.screen = s;
                    }
                    ui.add_space(2.0);
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    if !self.narrow {
                        ui.label(
                            RichText::new(format!("结构版本 {}", self.schema_version))
                                .size(10.5)
                                .color(p.text_muted),
                        );
                        ui.label(
                            RichText::new(format!(
                                "合同 {} · 拨款单 {}",
                                self.summaries.len(),
                                self.payments.len()
                            ))
                            .size(10.5)
                            .color(p.text_muted),
                        );
                        ui.add_space(4.0);
                        widgets::divider(ui, &p);
                    }
                });
            });
    }

    // ---------------------------------------------------------------- 状态栏

    fn status_bar(&mut self, ctx: &egui::Context) {
        let p = self.pal;
        egui::TopBottomPanel::bottom("status")
            .exact_height(34.0)
            .frame(
                egui::Frame::none()
                    .fill(p.surface)
                    .inner_margin(Margin::symmetric(18.0, 6.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let (text, tone) = match &self.toast {
                        Some(t) => (t.text.clone(), t.tone),
                        None => ("就绪".to_string(), Tone::Neutral),
                    };
                    let (fg, _) = tone.colors(&p);
                    ui.label(RichText::new("●").size(12.0).color(fg));
                    ui.label(RichText::new(text).size(12.5).color(p.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let role = self.role.map(|r| r.label()).unwrap_or("未登录");
                        ui.label(RichText::new(role).size(11.5).color(p.text_muted));
                    });
                });
            });
    }
}

/// 导航项：选中态为主色浅底 + 主色文字 + 左侧色条。
fn nav_item(
    ui: &mut egui::Ui,
    p: &Palette,
    text: &str,
    selected: bool,
    narrow: bool,
) -> egui::Response {
    let width = ui.available_width();
    let height = 38.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let hovered = resp.hovered();
    let bg = if selected {
        p.primary_soft
    } else if hovered {
        p.surface_alt
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, Rounding::same(9.0), bg);
    if selected {
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 1.0, rect.center().y - 8.0),
            egui::vec2(3.0, 16.0),
        );
        ui.painter()
            .rect_filled(bar, Rounding::same(2.0), p.primary);
    }
    let color = if selected { p.primary } else { p.text };
    let font = egui::FontId::proportional(if narrow { 12.5 } else { 13.5 });
    ui.painter().text(
        egui::pos2(
            rect.left() + if narrow { rect.width() / 2.0 } else { 16.0 },
            rect.center().y,
        ),
        if narrow {
            egui::Align2::CENTER_CENTER
        } else {
            egui::Align2::LEFT_CENTER
        },
        text,
        font,
        color,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 把渲染器截图写成 PPM（P6），避免为一个调试用途引入图片编码依赖。
fn write_ppm(path: &str, image: &egui::ColorImage) {
    let (w, h) = (image.size[0], image.size[1]);
    let mut buf = Vec::with_capacity(w * h * 3 + 32);
    buf.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
    for px in &image.pixels {
        buf.push(px.r());
        buf.push(px.g());
        buf.push(px.b());
    }
    let _ = std::fs::write(path, buf);
}

fn shorten_path(path: &str) -> String {
    let s = path.replace('\\', "/");
    if s.chars().count() <= 46 {
        return s;
    }
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() >= 3 {
        format!("…/{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        s
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // 取一次主窗口句柄，打印对话框用它做 owner（避免被主窗挡住）
        if self.hwnd == 0 {
            if let Ok(h) = frame.window_handle() {
                if let raw_window_handle::RawWindowHandle::Win32(w) = h.as_raw() {
                    self.hwnd = w.hwnd.get();
                }
            }
        }
        // 自动截图：`CTP_SHOT=<文件.ppm>`。走渲染器自己的截图通道，
        // 不依赖窗口是否在前台，也不受遮挡影响（PrintWindow 对 GPU 窗口常返回空白）。
        if self.shot.is_some() {
            let got = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = got {
                if let Some(job) = self.shot.take() {
                    write_ppm(&job.path, &img);
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            let job = self.shot.as_mut().expect("shot exists");
            if job.requested {
                ctx.request_repaint();
            } else if job.wait == 0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
                job.requested = true;
                ctx.request_repaint();
            } else {
                job.wait -= 1;
                ctx.request_repaint();
            }
        }
        // 冒烟模式：跑满若干帧后自行退出，并落一个结果文件。
        // 用于在无法截图的环境（窗口被遮挡 / 远程会话）里确认界面确实在正常渲染。
        if let Some(left) = self.smoke {
            if left == 0 {
                self.smoke = None;
                let body = format!(
                    "ok\nscreen={}\ncontracts={}\npayments={}\ndb={}\nerr={}\n",
                    self.screen.label(),
                    self.summaries.len(),
                    self.payments.len(),
                    self.db_path,
                    self.init_error.clone().unwrap_or_default()
                );
                let _ = std::fs::write(exe_dir().join("smoke.txt"), body);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            self.smoke = Some(left - 1);
            ctx.request_repaint();
        }
        // eframe 会在首帧按系统主题重置样式，这里在第一帧再应用一次自定义主题。
        if self.style_dirty {
            theme::apply(ctx, self.mode);
            self.style_dirty = false;
        }
        // 提示条 6 秒后淡出
        if let Some(t) = &self.toast {
            if t.at.elapsed() > Duration::from_secs(6) {
                self.toast = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(400));
            }
        }
        // R-44：打印成功提示 5 秒后自动关闭。request_repaint_after 让倒计时与
        // 自动关闭不依赖鼠标输入或窗口焦点，窗口静止时也会按时重绘。
        if self
            .print_success
            .as_ref()
            .is_some_and(print_success_expired)
        {
            dismiss_print_success(&mut self.print_success);
        } else if self.print_success.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        self.narrow = ctx.screen_rect().width() < 1100.0;

        // 未登录：只画登录页，不进入主界面（连顶栏导航都不出现）
        if self.role.is_none() {
            crate::screens::login::draw(self, ctx);
            return;
        }
        // 浏览用户不得停留在「操作日志」与「自检与设置」（含 CTP_SCREEN 直接启动到该页的情况）
        if (self.screen == Screen::Checks || self.screen == Screen::Logs) && !self.can_write() {
            self.screen = Screen::Dashboard;
        }
        // R-39：上一帧登记的耗时操作在这一帧执行，期间「处理中」状态挡住重复提交
        if let Some((kind, path)) = self.pending_import.take() {
            self.run_pending_import(kind, &path);
        }

        self.top_bar(ctx);
        self.status_bar(ctx);
        self.nav(ctx);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(self.pal.bg)
                    .inner_margin(Margin::same(18.0)),
            )
            .show(ctx, |ui| {
                if let Some(err) = self.init_error.clone() {
                    let p = self.pal;
                    widgets::card(&p).show(ui, |ui| {
                        ui.label(
                            RichText::new("数据库初始化失败")
                                .size(16.0)
                                .strong()
                                .color(p.danger),
                        );
                        ui.add_space(6.0);
                        ui.label(RichText::new(err).size(13.0).color(p.text));
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!("数据文件：{}", self.db_path))
                                .size(11.5)
                                .color(p.text_muted),
                        );
                    });
                    return;
                }
                screens::draw(self, ui);
            });

        dialogs::draw(self, ctx);

        // R-39：处理中状态。同步任务执行前显示一帧，并吃掉所有点击防止重复提交。
        if let Some(msg) = self.busy.clone() {
            let p = self.pal;
            let rect = ctx.screen_rect();
            egui::Area::new(egui::Id::new("busy_overlay"))
                .order(egui::Order::Tooltip)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    let _ = ui.allocate_response(rect.size(), egui::Sense::click());
                    widgets::card(&p).show(ui, |ui| {
                        ui.label(RichText::new(msg).size(14.0).strong().color(p.text));
                        ui.label(
                            RichText::new("处理中，请勿重复操作…")
                                .size(11.5)
                                .color(p.text_muted),
                        );
                    });
                });
            ctx.request_repaint();
        }
    }
}

fn main() -> eframe::Result<()> {
    install_panic_log();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1380.0, 880.0])
            // 最小尺寸需容纳「新建合同」弹窗（1060 宽、约 690 高），
            // 该页不做滚动，窗口再小就会裁切。
            .with_min_inner_size([1100.0, 720.0])
            .with_title("工程合同台账与拨款管理平台"),
        ..Default::default()
    };
    let result = eframe::run_native(
        "工程合同台账与拨款管理平台",
        opts,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    );
    // GUI 模式看不到 stderr：启动失败（缺图形驱动等）时落一份日志，避免「双击没反应」。
    if let Err(e) = &result {
        let dir = exe_dir().join("data");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(
            dir.join("startup-error.log"),
            format!("[{}] {e}\n", ctp_core::date::now_iso()),
        );
    }
    result
}
