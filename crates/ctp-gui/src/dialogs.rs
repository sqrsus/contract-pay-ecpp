//! 弹窗表单：新建合同、登记审计价、编辑合同金额、作废确认。
//!
//! 所有弹窗都复用同一套「卡片式」外观，并在提交按钮上做可用性判断：
//! 必填未填或数值非法时按钮置灰，避免用户点了才看到报错。

use crate::theme::Palette;
use crate::widgets;
use crate::App;
use ctp_app::service::{self, NewContract, NewPayment};
use ctp_core::contract::ContractType;
use ctp_core::date::{is_iso_date, today_iso};
use ctp_core::dict;
use ctp_core::money::Money;
use ctp_core::payment::PayMethod;
use ctp_core::rate::Rate;
use eframe::egui::{self, RichText};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    None,
    NewContract,
    AuditPrice,
    EditAmount,
}

/// 新建合同表单。
pub struct ContractForm {
    pub name: String,
    pub project_idx: usize,
    pub contractor_idx: usize,
    pub ctype_idx: usize,
    pub category_idx: usize,
    /// 0 = 不分类，其余对应 `ACCOUNTING_CATEGORIES[i-1]`
    pub acc_idx: usize,
    pub party_a: String,
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
}

impl Default for ContractForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            project_idx: 0,
            contractor_idx: 0,
            ctype_idx: 0,
            category_idx: 0,
            acc_idx: 0,
            party_a: String::new(),
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
        }
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
    }
    if app.confirm.is_some() {
        confirm_void(app, ctx);
    }
}

// ---------------------------------------------------------------- 新建合同

fn new_contract(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    let mut submit = false;
    let mut cancel = false;

    let projects: Vec<(String, String)> = app
        .projects
        .iter()
        .map(|x| (x.id.clone(), format!("{} {}", x.project_code, x.project_name)))
        .collect();
    let contractors: Vec<(String, String)> = app
        .contractors
        .iter()
        .map(|x| (x.id.clone(), x.contractor_name.clone()))
        .collect();

    modal("新建合同", &p)
        .default_width(980.0)
        .min_width(980.0)
        // 长表单用居中锚点在首帧尺寸未定时会顶出窗口，改为顶部固定留白
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 24.0))
        .show(ctx, |ui| {
            ui.set_width(940.0);
            ui.label(
                RichText::new("合同编号按「HT-年份-流水」自动生成；带 * 为必填。")
                    .size(12.0)
                    .color(p.text_muted),
            );
            ui.add_space(10.0);

            // 左栏基础字段、右栏长文本：整体压在 600px 高以内，不需要滚动
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
                            field(ui, &p, "发包人（甲方）", 300.0, &mut app.new_contract.party_a);
                            ui.end_row();

                            ui.vertical(|ui| {
                                widgets::field_label(ui, &p, "* 所属项目");
                                combo(
                                    ui,
                                    "project",
                                    300.0,
                                    app.new_contract.project_idx,
                                    projects.iter().map(|x| x.1.clone()).collect(),
                                    &mut app.new_contract.project_idx,
                                );
                            });
                            ui.vertical(|ui| {
                                widgets::field_label(ui, &p, "* 承包单位");
                                combo(
                                    ui,
                                    "contractor",
                                    300.0,
                                    app.new_contract.contractor_idx,
                                    contractors.iter().map(|x| x.1.clone()).collect(),
                                    &mut app.new_contract.contractor_idx,
                                );
                            });
                            ui.end_row();

                            ui.vertical(|ui| {
                                widgets::field_label(ui, &p, "合同类型");
                                let labels: Vec<String> =
                                    CONTRACT_TYPES.iter().map(|t| t.label().to_string()).collect();
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

                            ui.vertical(|ui| {
                                widgets::field_label(ui, &p, "会计分类");
                                let mut labels = vec!["（不分类）".to_string()];
                                labels.extend(
                                    dict::ACCOUNTING_CATEGORIES
                                        .iter()
                                        .map(|(c, n)| format!("{c} {n}")),
                                );
                                combo(
                                    ui,
                                    "acc",
                                    300.0,
                                    app.new_contract.acc_idx,
                                    labels,
                                    &mut app.new_contract.acc_idx,
                                );
                            });
                            field(
                                ui,
                                &p,
                                "* 签订日期（YYYY-MM-DD）",
                                300.0,
                                &mut app.new_contract.sign_date,
                            );
                            ui.end_row();

                            field(ui, &p, "* 合同金额（元）", 300.0, &mut app.new_contract.amount);
                            field(ui, &p, "工期（日历天）", 300.0, &mut app.new_contract.duration);
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
                            ui.horizontal(|ui| {
                                field(ui, &p, "质保金率 %", 138.0, &mut app.new_contract.retention);
                                field(ui, &p, "预付款率 %", 138.0, &mut app.new_contract.advance);
                            });
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

            // 校验
            let amount_ok = Money::parse_yuan(&app.new_contract.amount)
                .map(|m| m.cent() > 0)
                .unwrap_or(false);
            let name_ok = !app.new_contract.name.trim().is_empty();
            let terms_ok = !app.new_contract.terms.trim().is_empty();
            let date_ok = is_iso_date(&app.new_contract.sign_date);
            let base_ok = !projects.is_empty() && !contractors.is_empty();
            let ok = amount_ok && name_ok && terms_ok && date_ok && base_ok;

            ui.add_space(12.0);
            if !base_ok {
                ui.label(
                    RichText::new("缺少项目或参建单位：请先在「总览」页载入演示数据。")
                        .size(12.0)
                        .color(p.warn),
                );
                ui.add_space(6.0);
            }
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &p, "保存合同", ok).clicked() && ok {
                    submit = true;
                }
                if widgets::ghost_button(ui, &p, "取消").clicked() {
                    cancel = true;
                }
                if !ok {
                    ui.label(
                        RichText::new("请补全必填项并检查金额与日期格式")
                            .size(11.5)
                            .color(p.text_muted),
                    );
                }
            });
        });

    if cancel {
        app.dialog = Dialog::None;
    }
    if submit {
        let project_id = projects
            .get(app.new_contract.project_idx)
            .map(|x| x.0.clone())
            .unwrap_or_default();
        let contractor_id = contractors
            .get(app.new_contract.contractor_idx)
            .map(|x| x.0.clone())
            .unwrap_or_default();
        let amount = Money::parse_yuan(&app.new_contract.amount).unwrap_or(Money::ZERO);
        let ctype = CONTRACT_TYPES
            .get(app.new_contract.ctype_idx)
            .copied()
            .unwrap_or(ContractType::Construct);
        let cat_code = dict::CONTRACT_CATEGORIES
            .get(app.new_contract.category_idx)
            .map(|(c, _)| c.to_string())
            .unwrap_or_default();
        let acc = if app.new_contract.acc_idx == 0 {
            None
        } else {
            dict::ACCOUNTING_CATEGORIES
                .get(app.new_contract.acc_idx - 1)
                .map(|(c, _)| c.to_string())
        };
        let pricing = match app.new_contract.pricing_idx {
            0 => None,
            1 => Some(ctp_core::contract::PricingMethod::LumpSum),
            2 => Some(ctp_core::contract::PricingMethod::UnitPrice),
            _ => Some(ctp_core::contract::PricingMethod::CostPlus),
        };
        let duration = app.new_contract.duration.trim().parse::<i64>().unwrap_or(0);
        let retention = pct_to_bp(&app.new_contract.retention);
        let advance = pct_to_bp(&app.new_contract.advance);
        let name = app.new_contract.name.trim().to_string();
        let party_a = app.new_contract.party_a.trim().to_string();
        let sign_date = app.new_contract.sign_date.trim().to_string();
        let planned_start = opt(&app.new_contract.planned_start);
        let planned_end = opt(&app.new_contract.planned_end);
        let terms = app.new_contract.terms.trim().to_string();
        let remark = opt(&app.new_contract.remark);

        let created = app.exec(move |conn| {
            service::create_contract(
                conn,
                NewContract {
                    project_id,
                    contract_name: name,
                    contract_type: ctype,
                    contract_category_code: cat_code,
                    category_id: acc,
                    party_a,
                    contractor_id,
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
            )
        });
        match created {
            Ok(c) => {
                app.refresh();
                app.select(&c.id);
                app.new_contract = ContractForm::default();
                app.dialog = Dialog::None;
                app.success(format!("合同 {} 已建立", c.contract_no));
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
