//! 弹窗表单：新建合同、登记审计价、编辑合同金额、作废确认。
//!
//! 所有弹窗都复用同一套「卡片式」外观，并在提交按钮上做可用性判断：
//! 必填未填或数值非法时按钮置灰，避免用户点了才看到报错。

use crate::theme::Palette;
use crate::widgets::{self, Tone};
use crate::App;
use ctp_app::service::{self, NewContract, NewPayment};
use ctp_core::attachment::{Attachment, AttachmentKind};
use ctp_core::contract::ContractType;
use ctp_core::date::{is_iso_date, today_iso};
use ctp_core::dict;
use ctp_core::money::Money;
use ctp_core::payment::PayMethod;
use ctp_core::project::ContractorRole;
use ctp_core::rate::Rate;
use eframe::egui::{self, RichText};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    None,
    NewContract,
    AuditPrice,
    EditAmount,
    PaymentDetail,
    /// 合同附件管理（从合同详情卡的「附件 n/4」进来）
    Attachments,
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
    pub pricing_idx: usize,
    pub retention: String,
    pub advance: String,
    pub terms: String,
    pub remark: String,
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
            pricing_idx: 1,
            retention: "3".into(),
            advance: "0".into(),
            terms: "按月计量支付，进度款支付至已完工程量的 80%，竣工验收后支付至 97%，质保金 3% 满一年退还。".into(),
            remark: String::new(),
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
        widgets::field_label(ui, p, label);
        ui.add_sized(
            [w, 26.0],
            egui::TextEdit::singleline(value).margin(egui::Margin::symmetric(8.0, 4.0)),
        );
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
        Dialog::EditAmount => edit_amount(app, ctx),
        Dialog::PaymentDetail => payment_detail(app, ctx),
        Dialog::Attachments => attachments(app, ctx),
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
) -> Option<String> {
    ui.vertical(|ui| {
        widgets::field_label(ui, p, label);
        widgets::autocomplete(ui, id, p, text, hints, w, hint_text)
    })
    .inner
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
        .default_width(980.0)
        .min_width(980.0)
        // 长表单用居中锚点在首帧尺寸未定时会顶出窗口，改为顶部固定留白
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 24.0))
        .show(ctx, |ui| {
            ui.set_width(940.0);
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
                // ---------------- 第一步：基本信息（左栏字段 / 右栏长文本）
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(612.0, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::Grid::new("ct_form")
                                .num_columns(2)
                                .spacing([12.0, 8.0])
                                .show(ui, |ui| {
                                    field(ui, &p, "* 合同名称", 300.0, &mut app.new_contract.name);
                                    ac_field(
                                        ui,
                                        &p,
                                        "* 发包人（甲方）",
                                        "party_a",
                                        &mut app.new_contract.party_a,
                                        &owner_hints,
                                        "输入名称，可从已有业主中联想",
                                        300.0,
                                    );
                                    ui.end_row();

                                    ac_field(
                                        ui,
                                        &p,
                                        "* 所属项目",
                                        "project",
                                        &mut app.new_contract.project_text,
                                        &project_hints,
                                        "输入项目编码或名称",
                                        300.0,
                                    );
                                    ac_field(
                                        ui,
                                        &p,
                                        "* 承包单位",
                                        "contractor",
                                        &mut app.new_contract.contractor_text,
                                        &unit_hints,
                                        "输入名称，可从已有单位中联想",
                                        300.0,
                                    );
                                    ui.end_row();

                                    ui.vertical(|ui| {
                                        widgets::field_label(ui, &p, "合同类型");
                                        let labels: Vec<String> = CONTRACT_TYPES
                                            .iter()
                                            .map(|t| t.label().to_string())
                                            .collect();
                                        combo(
                                            ui,
                                            "ctype",
                                            300.0,
                                            app.new_contract.ctype_idx,
                                            labels,
                                            &mut app.new_contract.ctype_idx,
                                        );
                                    });
                                    ui.vertical(|ui| {
                                        widgets::field_label(ui, &p, "* 合同类别");
                                        let labels: Vec<String> = dict::CONTRACT_CATEGORIES
                                            .iter()
                                            .map(|(_, n)| n.to_string())
                                            .collect();
                                        combo(
                                            ui,
                                            "category",
                                            300.0,
                                            app.new_contract.category_idx,
                                            labels,
                                            &mut app.new_contract.category_idx,
                                        );
                                    });
                                    ui.end_row();

                                    field(
                                        ui,
                                        &p,
                                        "* 签订日期（YYYY-MM-DD）",
                                        300.0,
                                        &mut app.new_contract.sign_date,
                                    );
                                    field(
                                        ui,
                                        &p,
                                        "* 合同金额（元）",
                                        300.0,
                                        &mut app.new_contract.amount,
                                    );
                                    ui.end_row();

                                    field(
                                        ui,
                                        &p,
                                        "工期（日历天）",
                                        300.0,
                                        &mut app.new_contract.duration,
                                    );
                                    ui.vertical(|ui| {
                                        widgets::field_label(ui, &p, "计价方式");
                                        let labels: Vec<String> =
                                            PRICING.iter().map(|x| x.to_string()).collect();
                                        combo(
                                            ui,
                                            "pricing",
                                            300.0,
                                            app.new_contract.pricing_idx,
                                            labels,
                                            &mut app.new_contract.pricing_idx,
                                        );
                                    });
                                    ui.end_row();

                                    field(
                                        ui,
                                        &p,
                                        "计划开工（YYYY-MM-DD）",
                                        300.0,
                                        &mut app.new_contract.planned_start,
                                    );
                                    field(
                                        ui,
                                        &p,
                                        "计划竣工（YYYY-MM-DD）",
                                        300.0,
                                        &mut app.new_contract.planned_end,
                                    );
                                    ui.end_row();

                                    ui.horizontal(|ui| {
                                        field(
                                            ui,
                                            &p,
                                            "质保金率 %",
                                            138.0,
                                            &mut app.new_contract.retention,
                                        );
                                        field(
                                            ui,
                                            &p,
                                            "预付款率 %",
                                            138.0,
                                            &mut app.new_contract.advance,
                                        );
                                    });
                                    ui.label("");
                                    ui.end_row();
                                });
                        },
                    );

                    ui.add_space(16.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(300.0, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            widgets::field_label(ui, &p, "* 付款条款");
                            ui.add(
                                egui::TextEdit::multiline(&mut app.new_contract.terms)
                                    .desired_rows(9)
                                    .desired_width(300.0)
                                    .margin(egui::Margin::symmetric(8.0, 6.0)),
                            );
                            ui.add_space(8.0);
                            widgets::field_label(ui, &p, "备注");
                            ui.add(
                                egui::TextEdit::multiline(&mut app.new_contract.remark)
                                    .desired_rows(7)
                                    .desired_width(300.0)
                                    .margin(egui::Margin::symmetric(8.0, 6.0)),
                            );
                        },
                    );
                });

                ui.add_space(10.0);
                ui.label(
                    RichText::new(
                        "所属项目、承包单位、发包人可直接输入新名称：库里没有会自动建档，项目编码自动生成。",
                    )
                    .size(11.5)
                    .color(p.text_muted),
                );
                ui.add_space(10.0);
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
                            ui.set_width(908.0);
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
        let advance = pct_to_bp(&f.advance);
        let sign_date = f.sign_date.trim().to_string();
        let planned_start = opt(&f.planned_start);
        let planned_end = opt(&f.planned_end);
        let terms = f.terms.trim().to_string();
        let remark = opt(&f.remark);
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
                    pricing_method: pricing,
                    retention_rate_bp: retention,
                    advance_rate_bp: advance,
                    payment_terms: terms,
                    remark,
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
                    field(ui, &p, "审定日期", 300.0, &mut app.audit_form.date);
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
    let mut goto = false;

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
            widgets::kv(
                ui,
                &p,
                "银行流水号",
                pm.bank_serial_no.as_deref().unwrap_or("—"),
            );
            widgets::kv(ui, &p, "付款账户", pm.pay_account.as_deref().unwrap_or("—"));
            widgets::kv(
                ui,
                &p,
                "收款账户",
                pm.receive_account.as_deref().unwrap_or("—"),
            );
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
                if contract.is_some() && widgets::ghost_button(ui, &p, "查看所属合同").clicked() {
                    goto = true;
                }
            });
        });

    if close {
        app.dialog = Dialog::None;
        app.pay_detail = None;
    } else if goto {
        app.dialog = Dialog::None;
        app.pay_detail = None;
        app.goto_contract(&pm.contract_id);
    }
}

// ---------------------------------------------------------------- 合同附件

/// 把字节落到临时目录后用系统默认程序打开。
///
/// 附件存在库里，没有「原文件路径」可复用，所以每次打开都先物化一份临时副本。
fn open_external(path: &std::path::Path) -> Result<(), String> {
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
