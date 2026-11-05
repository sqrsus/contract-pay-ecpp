//! 弹窗表单：新建合同、登记审计价、编辑合同金额、作废确认。
//!
//! 所有弹窗都复用同一套「卡片式」外观，并在提交按钮上做可用性判断：
//! 必填未填或数值非法时按钮置灰，避免用户点了才看到报错。

use crate::theme::Palette;
use crate::widgets::{self, Tone};
use crate::App;
use ctp_app::service::{self, NewContract, NewPayment};
use ctp_core::attachment::{Attachment, AttachmentKind};
use ctp_core::contract::{BidMethod, ContractType};
use ctp_core::date::{is_iso_date, today_iso};
use ctp_core::dict;
use ctp_core::money::Money;
use ctp_core::payment::PayMethod;
use ctp_core::project::ContractorRole;
use ctp_core::rate::Rate;
use ctp_store::repo;
use eframe::egui::{self, RichText};

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
pub struct AmountForm {
    pub amount: String,
    pub reason: String,
}

impl Default for AmountForm {
    fn default() -> Self {
        Self {
            amount: String::new(),
            reason: String::new(),
        }
    }
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
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        widgets::field_label(ui, p, label);
        widgets::date_input(ui, id, p, value, w);
    });
}

/// 下拉框（索引式）。
fn combo(ui: &mut egui::Ui, id: &str, width: f32, current: usize, labels: Vec<String>, sel: &mut usize) {
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

pub fn draw(app: &mut App, ctx: &egui::Context) {
    // 弹窗打开时压暗底图，把注意力收到弹窗上。
    // 该图层在弹窗图层之前创建，因此始终绘制在弹窗下方；它不参与交互，点击可正常穿透到按钮。
    if app.dialog != Dialog::None || app.confirm.is_some() {
        let alpha = if app.mode.is_dark() { 130 } else { 55 };
        // 画在背景层末尾：盖住主界面，但仍在 Order::Middle 的窗口之下
        ctx.layer_painter(egui::LayerId::background())
            .rect_filled(
                ctx.screen_rect(),
                egui::Rounding::ZERO,
                egui::Color32::from_black_alpha(alpha),
            );
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
    }
    if app.confirm.is_some() {
        confirm_void(app, ctx);
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
            &["pdf", "doc", "docx", "xls", "xlsx", "jpg", "jpeg", "png", "txt"],
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
            ui.label(
                RichText::new(format!("{no} {label}"))
                    .size(12.0)
                    .color(fg),
            );
        });
}

/// 带字段标签的联想输入框。
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
    let project_hints: Vec<String> = app
        .projects
        .iter()
        .map(|x| format!("{} {}", x.project_code, x.project_name))
        .collect();
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
                                    ac_field(ui, &p, "* 所属项目", "project", &mut app.new_contract.project_text, &project_hints, "输入编码或名称后联想", 218.0, widgets::SuggestMode::TypedOnly);
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
                                    date_field(ui, &p, "计划交工", "planned_end", 218.0, &mut app.new_contract.planned_end);
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
// ---------------------------------------------------------------- 审计价

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
            widgets::date_input(ui, "delivery_date_dialog", &p, &mut app.delivery_form, 260.0);

            let ok = is_iso_date(&app.delivery_form);
            if !ok {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("请选择合法日期（YYYY-MM-DD）")
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
            let ok = amount_ok && date_ok && reason_ok && id.is_some();

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
    let (no, cur) = app
        .selected_summary()
        .map(|s| (s.contract_no.clone(), s.total_amount.format_yuan()))
        .unwrap_or_default();

    modal(format!("修改合同金额 · {no}"), &p)
        .default_width(480.0)
        .show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(
                RichText::new(format!("当前合同金额 {cur} 元。修改会留痕（BR-22）；已审定合同不改基准。"))
                    .size(12.0)
                    .color(p.text_muted),
            );
            ui.add_space(10.0);
            field(ui, &p, "* 新合同金额（元）", 440.0, &mut app.amount_form.amount);
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
            let ok = amount_ok && !app.amount_form.reason.trim().is_empty() && id.is_some();
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
                    app.success(format!("合同金额已改为 {} 元", c.total_amount().format_yuan()));
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
                if contract.is_some()
                    && widgets::ghost_button(ui, &p, "查看合同详情").clicked()
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
        return;
    }
    let sheets = app.print_vouchers.len();
    let total_pages = sheets * ctp_app::paper::PAGES_PER_VOUCHER;
    let page = app.print_page.min(total_pages.saturating_sub(1));
    let first_no = app.print_vouchers[0].payment_no.clone();
    let mut close = false;
    let mut do_print = false;

    // ---- 当前页位图（带缓存，只在换页/首次时渲染）
    if app.print_tex.as_ref().map(|(_, i)| *i) != Some(page) {
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
                app.error(format!("渲染预览失败：{e}"));
                app.dialog = Dialog::None;
                return;
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
            }
            let can_next = page + 1 < total_pages;
            if widgets::ghost_button_sm_on(ui, &p, "下一页", can_next).clicked() && can_next {
                app.print_page = page + 1;
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
            if widgets::primary_button(ui, &p, "打印", true)
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
fn ro_field(
    ui: &mut egui::Ui,
    p: &Palette,
    label: &str,
    value: &str,
    w: f32,
) {
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
    let pricing = c.pricing_method.map(|x| x.label()).unwrap_or("—");

    let mut close = false;
    let mut material: Option<AttachmentKind> = None;

    modal(format!("合同详情 · {}", c.contract_no), &p)
        .default_width(1060.0)
        .min_width(1060.0)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 16.0))
        .show(ctx, |ui| {
            ui.set_width(1020.0);
            ui.label(
                RichText::new("只读视图；如需修改请关闭后从对应操作入口处理。")
                    .size(12.0)
                    .color(p.text_muted),
            );
            ui.add_space(8.0);

            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(486.0);
                    form_card(&p).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        widgets::section_title(ui, &p, "合同主体", "");
                        egui::Grid::new("ro_subject")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ro_field(ui, &p, "合同名称", &c.contract_name, 218.0);
                                ro_field(ui, &p, "发包人（甲方）", &c.party_a, 218.0);
                                ui.end_row();
                                ro_field(ui, &p, "所属项目", &project_text, 218.0);
                                ro_field(ui, &p, "承包单位", &contractor, 218.0);
                                ui.end_row();
                                ro_field(
                                    ui,
                                    &p,
                                    "项目负责人",
                                    c.project_leader.as_deref().unwrap_or("—"),
                                    218.0,
                                );
                                ro_field(
                                    ui,
                                    &p,
                                    "联系方式",
                                    c.contact_info.as_deref().unwrap_or("—"),
                                    218.0,
                                );
                                ui.end_row();
                            });
                    });
                    ui.add_space(10.0);
                    form_card(&p).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        widgets::section_title(ui, &p, "招标与工期", "");
                        egui::Grid::new("ro_bid")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ro_field(
                                    ui,
                                    &p,
                                    "招标方式",
                                    c.bid_method.map(|b| b.label()).unwrap_or("—"),
                                    218.0,
                                );
                                ro_field(
                                    ui,
                                    &p,
                                    "开标时间",
                                    c.open_bid_date.as_deref().unwrap_or("—"),
                                    218.0,
                                );
                                ui.end_row();
                                ro_field(ui, &p, "招标代理", &agent, 218.0);
                                ro_field(
                                    ui,
                                    &p,
                                    "工期（日历天）",
                                    &c.duration_days.to_string(),
                                    218.0,
                                );
                                ui.end_row();
                                ro_field(
                                    ui,
                                    &p,
                                    "计划开工",
                                    c.planned_start.as_deref().unwrap_or("—"),
                                    218.0,
                                );
                                ro_field(
                                    ui,
                                    &p,
                                    "计划交工",
                                    c.planned_end.as_deref().unwrap_or("—"),
                                    218.0,
                                );
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
                        egui::Grid::new("ro_price")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ro_field(ui, &p, "合同类型", c.contract_type.label(), 218.0);
                                ro_field(ui, &p, "合同类别", category, 218.0);
                                ui.end_row();
                                ro_field(ui, &p, "签订日期", &c.sign_date, 218.0);
                                ro_field(
                                    ui,
                                    &p,
                                    "合同金额（元）",
                                    &c.original_amount_cent.format_yuan(),
                                    218.0,
                                );
                                ui.end_row();
                                ro_field(ui, &p, "计价方式", pricing, 218.0);
                                ro_field(
                                    ui,
                                    &p,
                                    "质保金率 %",
                                    &format!(
                                        "{:.2}",
                                        c.retention_rate_bp as f32 / 100.0
                                    ),
                                    218.0,
                                );
                                ui.end_row();
                                ro_field(
                                    ui,
                                    &p,
                                    "交工日期",
                                    c.delivery_date.as_deref().unwrap_or("—"),
                                    218.0,
                                );
                                ui.label("");
                                ui.end_row();
                            });
                        ui.add_space(6.0);
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
                                RichText::new(kind.label()).size(13.0).strong().color(p.text),
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
        .map(|k| {
            (
                *k,
                list.iter().find(|a| a.kind_code == k.code()).cloned(),
            )
        })
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
                                            RichText::new(&a.file_name)
                                                .size(12.5)
                                                .color(p.primary),
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
                                |ui| {
                                    match att {
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
                                            if widgets::ghost_button_sm(ui, &p, "上传").clicked()
                                            {
                                                op_pick = Some(*kind);
                                            }
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
        match app.exec(move |conn| service::attachment_bytes(conn, &id)) {
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
        match app.exec(move |conn| service::attachment_bytes(conn, &id)) {
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

pub fn new_payment_input(contract_id: &str, amount: Money, date: &str, reason: &str, idx: usize) -> NewPayment {
    let mut input = NewPayment::new(contract_id, amount, date);
    input.reason = Some(reason.to_string());
    input.pay_method = PAY_METHODS.get(idx).copied().unwrap_or(PayMethod::Transfer);
    input
}
