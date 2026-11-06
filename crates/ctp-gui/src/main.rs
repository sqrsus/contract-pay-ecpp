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
use rusqlite::Connection;
use raw_window_handle::HasWindowHandle;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use dialogs::{AmountForm, AuditForm, ContractForm, Dialog, PayForm};
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
            Screen::Checks => "口径自检、超付容差、管理密码",
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
    VoidPayment(Payment),
    VoidContract(Contract),
}

/// 冒烟/截图任务：`CTP_SELFTEST=1` 跑满帧退出；`CTP_SHOT=路径` 保存一帧界面。
pub struct ShotJob {
    pub path: String,
    /// 还要等多少帧再请求截图（等字体与数据就绪）
    pub wait: u32,
    pub requested: bool,
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
    pub tolerance_bp: i32,
    pub schema_version: String,

    pub search: String,
    pub status_filter: Option<ContractStatus>,
    pub selected: Option<String>,
    pub detail: Option<Contract>,
    /// 合同台账里展开拨款明细的合同（点击合同名称切换）
    pub expanded_contract: Option<String>,

    pub pay_contract: Option<String>,
    /// 拨款管理页合同搜索框的文本（可输入编号或名称，实时匹配切换）
    pub pay_contract_text: String,
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
    pub tol_input: String,

    // ---------------- 登录与权限（角色只在进程内有效，关掉程序即失效）
    /// 当前登录身份；`None` 表示停在登录页
    pub role: Option<ctp_app::auth::Role>,
    /// 登录页的密码输入框
    pub login_pw: String,
    /// 登录页的即时错误提示
    pub login_err: Option<String>,
    /// 「管理登录」是否已展开密码框
    pub login_admin: bool,
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
    pub summary_key: Option<(
        ctp_core::ledger::SummaryDim,
        String,
        String,
        Option<bool>,
        Option<String>,
        String,
    )>,

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
            tolerance_bp: 0,
            schema_version: String::new(),
            search: String::new(),
            status_filter: None,
            selected: None,
            detail: None,
            expanded_contract: None,
            pay_contract: None,
            pay_contract_text: String::new(),
            pay_form: PayForm::default(),
            pay_detail: None,
            print_vouchers: Vec::new(),
            print_ids: Vec::new(),
            print_page: 0,
            print_tex: None,
            hwnd: 0,
            print_sel: std::collections::BTreeSet::new(),
            new_contract: ContractForm::default(),
            new_contract_flash: None,
            modal_contract: None,
            audit_form: AuditForm::default(),
            delivery_form: String::new(),
            amount_form: AmountForm::default(),
            tol_input: String::new(),
            role: None,
            login_pw: String::new(),
            login_err: None,
            login_admin: false,
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
                self.sync_pay_text();
                self.screen = Screen::Payments;
            }
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
                    self.confirm = Some(Confirm::VoidPayment(pm));
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
                    self.sync_pay_text();
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
                    self.sync_pay_text();
                    self.screen = Screen::Payments;
                    let ids: Vec<String> = self
                        .payments
                        .iter()
                        .filter(|p| {
                            p.contract_id == pm.contract_id
                                && p.status == PaymentStatus::Paid
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
    pub fn refresh(&mut self) {
        let Some(conn) = self.db.as_ref() else {
            return;
        };
        if let Ok(v) = repo::summaries(conn) {
            self.summaries = v;
        }
        if let Ok(v) = repo::list_projects(conn) {
            self.projects = v;
        }
        if let Ok(v) = repo::list_contractors(conn) {
            self.contractors = v;
        }
        if let Ok(v) = repo::list_party_a_names(conn) {
            self.party_a_names = v;
        }
        if let Ok(v) = repo::list_payments(conn, None) {
            self.payments = v;
        }
        if let Ok(v) = repo::category_summary(conn) {
            self.categories = v;
        }
        if let Ok(v) = repo::period_summary(conn) {
            self.periods = v;
        }
        if let Ok(bp) = db::tolerance(conn) {
            self.tolerance_bp = bp.bp();
            self.tol_input = bp.bp().to_string();
        }
        if let Ok(Some(v)) = db::schema_version(conn) {
            self.schema_version = v;
        }
        // 写操作之后日志一定有新条目：置空快照让操作日志页下一帧重查
        self.log_key = None;

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
        if self.pay_contract.is_none() {
            self.pay_contract = self.summaries.first().map(|s| s.contract_id.clone());
        }
        self.sync_pay_text();
    }

    /// 合同在搜索框里的展示文本：`编号 名称`。
    pub fn pay_contract_label(&self) -> Option<String> {
        let id = self.pay_contract.as_ref()?;
        self.summaries
            .iter()
            .find(|s| &s.contract_id == id)
            .map(|s| format!("{} {}", s.contract_no, s.contract_name))
    }

    /// 把当前选中合同回填到搜索框（切换合同 / 刷新数据后调用）。
    pub fn sync_pay_text(&mut self) {
        self.pay_contract_text = self.pay_contract_label().unwrap_or_default();
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
        self.sync_pay_text();
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
            let bytes = ctp_app::service::attachment_bytes(conn, &a.id)
                .map_err(|e| e.to_string())?;
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

    /// 批量打印入口：拨款管理页勾选的若干张，按列表顺序成页（一张两页）。
    pub fn open_batch_print(&mut self) {
        if self.print_sel.is_empty() {
            self.error("请先勾选要打印的拨款单");
            return;
        }
        // 以列表顺序为准：勾选先后不影响成品里的顺序；只取当前合同下的单子
        let cid = self.pay_contract.clone();
        let ids: Vec<String> = self
            .payments
            .iter()
            .filter(|p| {
                Some(&p.contract_id) == cid.as_ref() && self.print_sel.contains(&p.id)
            })
            .map(|p| p.id.clone())
            .collect();
        if ids.is_empty() {
            self.print_sel.clear();
            self.error("勾选的拨款单已不在当前列表中，请重新勾选");
            return;
        }
        self.print_ids = ids;
        self.load_print_vouchers();
    }

    /// 解析版面数据并打开预览。取数与打印同一入口，不存在第二套口径。
    fn load_print_vouchers(&mut self) {
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
                self.dialog = Dialog::PrintPreview;
            }
            Some(Ok(_)) => self.error("没有可打印的拨款单"),
            Some(Err(e)) => self.error(format!("准备打印数据失败：{e}")),
            None => self.error("数据库未打开"),
        }
    }

    /// 打印：直接弹系统「打印」对话框并出纸，不经过 Word / PDF。
    ///
    /// 用户取消对话框不算错误（`Ok(0)`）；其余失败一律报错中止。
    pub fn print_now(&mut self) {
        if self.print_vouchers.is_empty() {
            self.error("没有可打印的拨款单");
            return;
        }
        let vs = self.print_vouchers.clone();
        let name = ctp_app::print::doc_name(&vs);
        match ctp_app::print_vouchers_now(self.hwnd, &vs, &name) {
            Ok(0) => {}
            Ok(pages) => self.success(format!(
                "已向打印机发送 {} 张单据、共 {} 页",
                vs.len(),
                pages
            )),
            Err(e) => self.error(e.to_string()),
        }
    }

    /// 报表导出：弹保存框 → 从视图构造 `ReportResult` → 写 .xlsx → 落 EXPORT 审计日志。
    ///
    /// 屏幕表格与导出读的是同一份 `ReportResult` 构造逻辑（文档 4.4.2）。
    pub fn export_report(&mut self, kind: ReportKind) {
        // 闭包把不可变借用限制在内部，结束后才能安全地写 toast。
        let outcome: Result<(usize, std::path::PathBuf), String> = (|| {
            let conn = self.db.as_ref().ok_or_else(|| "数据库未打开".to_string())?;
            let file_name = ctp_app::service::report_of(conn, kind)
                .map(|r| r.default_file_name())
                .unwrap_or_else(|_| format!("{}.xlsx", kind.file_stem()));
            let path = rfd::FileDialog::new()
                .set_file_name(&file_name)
                .add_filter("Excel 工作簿", &["xlsx"])
                .save_file()
                // 用户取消保存框：用空串标记，不弹错误提示
                .ok_or_else(String::new)?;
            let (rows, _) = ctp_app::service::export_report(conn, kind, &path, "界面用户")
                .map_err(|e| e.to_string())?;
            Ok((rows, path))
        })();

        match outcome {
            Ok((rows, path)) => self.success(format!(
                "已导出「{}」{} 行 → {}",
                kind.title(),
                rows,
                path.display()
            )),
            Err(e) if e.is_empty() => {}
            Err(e) => self.error(format!("导出失败：{e}")),
        }
    }

    /// 导出当前汇总结果（区间由界面决定，不能走 `report_of` 的默认区间）。
    pub fn export_summary(&mut self) {
        let Some(result) = self.summary.clone() else {
            self.error("还没有汇总结果，先选好区间");
            return;
        };
        let outcome: Result<(usize, std::path::PathBuf), String> = (|| {
            let conn = self.db.as_ref().ok_or_else(|| "数据库未打开".to_string())?;
            let path = rfd::FileDialog::new()
                .set_file_name(&result.default_file_name())
                .add_filter("Excel 工作簿", &["xlsx"])
                .save_file()
                .ok_or_else(String::new)?;
            let (rows, _) =
                ctp_app::service::export_result(conn, &result, &path, "界面用户")
                    .map_err(|e| e.to_string())?;
            Ok((rows, path))
        })();
        match outcome {
            Ok((rows, path)) => self.success(format!(
                "已导出「{}」{} 行 → {}",
                result.title,
                rows,
                path.display()
            )),
            Err(e) if e.is_empty() => {}
            Err(e) => self.error(format!("导出失败：{e}")),
        }
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

    /// 当前身份能否查看操作日志。现阶段与写权限一致（管理员）；
    /// 将来加「授权角色」时只需改这一处。
    pub fn can_view_logs(&self) -> bool {
        self.can_write()
    }

    /// 权限不足的统一提示文案。
    pub const NO_PERMISSION: &'static str = "无权限：浏览模式仅可查看，请用「管理登录」进入后再操作";

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
        egui::TopBottomPanel::top("top_bar")
            .exact_height(62.0)
            .frame(
                egui::Frame::none()
                    .fill(p.surface)
                    .inner_margin(Margin::symmetric(20.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(34.0, 34.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(rect, Rounding::same(9.0), p.primary);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "合",
                        egui::FontId::proportional(17.0),
                        p.on_primary,
                    );
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("工程合同台账与拨款管理平台").size(16.0).strong().color(p.text));
                        ui.label(
                            RichText::new(format!("{} · {}", self.screen.label(), self.screen.hint()))
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
                        let (role_text, role_tone) = match self.role {
                            Some(r) if r.can_write() => (r.label(), Tone::Primary),
                            Some(r) => (r.label(), Tone::Neutral),
                            None => ("未登录", Tone::Neutral),
                        };
                        widgets::badge(ui, &p, role_text, role_tone);
                        ui.add_space(8.0);
                        let theme_text = if self.mode.is_dark() { "浅色" } else { "深色" };
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

        if logout {
            self.role = None;
            self.dialog = Dialog::None;
            self.confirm = None;
            self.modal_contract = None;
            self.screen = Screen::Dashboard;
            self.login_pw.clear();
            self.login_err = None;
            self.login_admin = false;
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
                    ui.label(
                        RichText::new("功能导航")
                            .size(11.0)
                            .color(p.text_muted),
                    );
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
                        ui.label(
                            RichText::new(format!(
                                "{role} · 超付容差 {} bp",
                                self.tolerance_bp
                            ))
                            .size(11.5)
                            .color(p.text_muted),
                        );
                    });
                });
            });
    }
}

/// 导航项：选中态为主色浅底 + 主色文字 + 左侧色条。
fn nav_item(ui: &mut egui::Ui, p: &Palette, text: &str, selected: bool, narrow: bool) -> egui::Response {
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
        ui.painter().rect_filled(bar, Rounding::same(2.0), p.primary);
    }
    let color = if selected { p.primary } else { p.text };
    let font = egui::FontId::proportional(if narrow { 12.5 } else { 13.5 });
    ui.painter().text(
        egui::pos2(rect.left() + if narrow { rect.width() / 2.0 } else { 16.0 }, rect.center().y),
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

fn shorten_path(path: &str) -> String {    let s = path.replace('\\', "/");
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
                    self.hwnd = w.hwnd.get() as isize;
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
                        ui.label(RichText::new("数据库初始化失败").size(16.0).strong().color(p.danger));
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
