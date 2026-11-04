//! 拨款管理页：合同可付额度、登记表单、拨款明细。

use crate::screens::page_header;
use crate::widgets::{self, Tone};
use crate::{App, Confirm};
use ctp_app::service;
use ctp_core::date::is_iso_date;
use ctp_core::money::Money;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    page_header(
        ui,
        &p,
        "拨款管理",
        "登记即校验额度：超过「有效基准 ×（1 + 容差）」会被拦截，提示可付余额",
    );

    if app.summaries.is_empty() {
        widgets::card(&p).show(ui, |ui| {
            widgets::empty_state(ui, &p, "还没有合同，先去「合同台账」新建或载入演示数据。");
        });
        return;
    }

    // ---------------- 合同选择
    let current = app
        .pay_contract
        .clone()
        .or_else(|| app.summaries.first().map(|s| s.contract_id.clone()));
    let current_label = current
        .as_ref()
        .and_then(|id| app.summaries.iter().find(|s| &s.contract_id == id))
        .map(|s| format!("{} {}", s.contract_no, s.contract_name))
        .unwrap_or_else(|| "请选择合同".into());

    let mut pick: Option<String> = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("合同").size(12.5).color(p.text_muted));
        egui::ComboBox::from_id_salt("pay_contract")
            .width(420.0)
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                for s in &app.summaries {
                    let sel = current.as_deref() == Some(&s.contract_id);
                    if ui
                        .selectable_label(sel, format!("{} {}", s.contract_no, s.contract_name))
                        .clicked()
                    {
                        pick = Some(s.contract_id.clone());
                    }
                }
            });
        if let Some(id) = pick {
            app.pay_contract = Some(id);
        }
    });

    let Some(cid) = app.pay_contract.clone() else {
        return;
    };
    let Some(s) = app.summaries.iter().find(|s| s.contract_id == cid) else {
        return;
    };
    let quota = app
        .db
        .as_ref()
        .and_then(|c| service::quota_of(c, &cid).ok());
    let (base, paid, avail, rate, tol_bp) = match &quota {
        Some(q) => (
            q.base_amount.format_yuan(),
            q.paid_amount.format_yuan(),
            q.available_amount.format_yuan(),
            q.pay_rate.percent_string(),
            q.tolerance.bp(),
        ),
        None => (
            s.base_amount.format_yuan(),
            s.paid_amount.format_yuan(),
            s.available_amount.format_yuan(),
            s.pay_rate.percent_string(),
            app.tolerance_bp,
        ),
    };
    let avail_cent = quota.as_ref().map(|q| q.available_amount.cent()).unwrap_or(0);
    let overdue = quota.as_ref().map(|q| q.available_amount.cent() < 0).unwrap_or(false);

    ui.add_space(10.0);

    // ---------------- 额度卡片
    let avail_w = ui.available_width();
    let gap = 12.0_f32;
    let cols = (((avail_w + gap) / 252.0).floor() as usize).clamp(1, 4);
    let w = (avail_w - gap * (cols as f32 - 1.0)) / cols as f32;
    let cards: Vec<(&str, String, String, Tone)> = vec![
        ("有效计价基准", format!("{base} 元"), format!("容差 {tol_bp} bp"), Tone::Neutral),
        ("累计已付", format!("{paid} 元"), rate.clone(), Tone::Primary),
        (
            "可付额度",
            format!("{avail} 元"),
            if overdue { "已超额，需先处理" } else { "上限 − 已付" }.to_string(),
            if overdue { Tone::Danger } else { Tone::Success },
        ),
        (
            "已登记单据",
            format!("{} 笔", app.payments.iter().filter(|x| x.contract_id == cid).count()),
            format!(
                "其中作废 {} 笔",
                app.payments
                    .iter()
                    .filter(|x| {
                        x.contract_id == cid
                            && x.status == ctp_core::payment::PaymentStatus::Void
                    })
                    .count()
            ),
            Tone::Neutral,
        ),
    ];
    ui.scope(|ui| {
        ui.style_mut().spacing.item_spacing.x = gap;
        for row in cards.chunks(cols) {
            ui.horizontal(|ui| {
                for (label, value, sub, tone) in row {
                    widgets::kpi(ui, &p, w, label, value, sub, *tone);
                }
            });
            ui.add_space(10.0);
        }
    });

    ui.add_space(14.0);

    // ---------------- 登记表单 + 明细
    let mut submit = false;
    let mut void_target: Option<ctp_core::payment::Payment> = None;

    ui.horizontal_top(|ui| {
        let form_w = if avail_w > 900.0 { 344.0 } else { avail_w };
        ui.allocate_ui_with_layout(
            egui::vec2(form_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                widgets::card(&p).show(ui, |ui| {
                    ui.set_width(form_w - 34.0);
                    widgets::section_title(ui, &p, "登记拨款单", "单级登记，无需审批");
                    ui.add_space(6.0);

                    widgets::field_label(ui, &p, "* 本次金额（元）");
                    ui.add_sized(
                        [form_w - 40.0, 30.0],
                        egui::TextEdit::singleline(&mut app.pay_form.amount)
                            .hint_text("例如 1250000.00")
                            .margin(egui::Margin::symmetric(8.0, 4.0)),
                    );

                    // 快捷填充：可付额度的一半 / 全额
                    ui.horizontal(|ui| {
                        let half = avail_cent / 2;
                        if ui.small_button("可付一半").clicked() && half > 0 {
                            app.pay_form.amount = Money::from_cent(half).format_yuan();
                        }
                        if ui.small_button("填满额度").clicked() && avail_cent > 0 {
                            app.pay_form.amount = Money::from_cent(avail_cent).format_yuan();
                        }
                    });

                    ui.add_space(6.0);
                    widgets::field_label(ui, &p, "* 开单日期");
                    ui.add_sized(
                        [form_w - 40.0, 30.0],
                        egui::TextEdit::singleline(&mut app.pay_form.date)
                            .margin(egui::Margin::symmetric(8.0, 4.0)),
                    );

                    ui.add_space(6.0);
                    widgets::field_label(ui, &p, "事由");
                    ui.add_sized(
                        [form_w - 40.0, 30.0],
                        egui::TextEdit::singleline(&mut app.pay_form.reason)
                            .hint_text("例如 2026 年 7 月进度款")
                            .margin(egui::Margin::symmetric(8.0, 4.0)),
                    );

                    ui.add_space(6.0);
                    widgets::field_label(ui, &p, "支付方式");
                    let mut labels: Vec<String> = Vec::new();
                    for m in crate::dialogs::pay_methods() {
                        labels.push(m.label().to_string());
                    }
                    egui::ComboBox::from_id_salt("pay_method")
                        .width(form_w - 40.0)
                        .selected_text(crate::dialogs::pay_method_label(app.pay_form.method_idx))
                        .show_ui(ui, |ui| {
                            for (i, l) in labels.iter().enumerate() {
                                ui.selectable_value(&mut app.pay_form.method_idx, i, l);
                            }
                        });

                    // 校验
                    let amount = Money::parse_yuan(&app.pay_form.amount).ok();
                    let amount_ok = amount.map(|m| m.cent() > 0).unwrap_or(false);
                    let date_ok = is_iso_date(&app.pay_form.date);
                    let over = amount.map(|m| m.cent() > avail_cent).unwrap_or(false);
                    let ok = amount_ok && date_ok && !over;

                    ui.add_space(9.0);
                    if over {
                        widgets::soft_box(&p, p.warn_soft).show(ui, |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "超出可付额度：本次 {} 元 > 可付 {} 元",
                                    amount.map(|m| m.format_yuan()).unwrap_or_default(),
                                    avail
                                ))
                                .size(11.5)
                                .color(p.warn),
                            );
                        });
                        ui.add_space(6.0);
                    } else if !date_ok {
                        ui.label(
                            RichText::new("日期格式应为 YYYY-MM-DD")
                                .size(11.5)
                                .color(p.warn),
                        );
                        ui.add_space(6.0);
                    }

                    if widgets::primary_button(ui, &p, "登记拨款单", ok).clicked() && ok {
                        submit = true;
                    }
                });
            },
        );

        if avail_w > 900.0 {
            ui.add_space(12.0);
            let list_w = avail_w - form_w - 12.0;
            ui.allocate_ui_with_layout(
                egui::vec2(list_w, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    widgets::card(&p).show(ui, |ui| {
                        ui.set_width(list_w - 34.0);
                        widgets::section_title(ui, &p, "拨款明细", "作废单保留记录、不计入已付净额");
                        ui.add_space(6.0);
                        let rows: Vec<_> = app
                            .payments
                            .iter()
                            .filter(|pm| pm.contract_id == cid)
                            .collect();
                        if rows.is_empty() {
                            widgets::empty_state(ui, &p, "该合同还没有拨款记录");
                            return;
                        }
                        ui.spacing_mut().item_spacing.x = 20.0;
                        TableBuilder::new(ui)
                            .striped(true)
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(Column::auto().at_least(112.0))
                            .column(Column::auto().at_least(86.0))
                            .column(Column::auto().at_least(100.0))
                            .column(Column::remainder().at_least(96.0))
                            .column(Column::auto().at_least(62.0))
                            .column(Column::auto().at_least(48.0))
                            .header(26.0, |mut h| {
                                for t in ["拨款单号", "开单日期", "金额", "事由", "状态", "操作"] {
                                    h.col(|ui| {
                                        ui.label(RichText::new(t).size(12.0).color(p.text_muted));
                                    });
                                }
                            })
                            .body(|mut body| {
                                for pm in rows.iter().copied() {
                                    body.row(30.0, |mut row| {
                                        row.col(|ui| {
                                            ui.label(
                                                RichText::new(&pm.payment_no)
                                                    .size(12.0)
                                                    .color(p.primary),
                                            );
                                        });
                                        row.col(|ui| {
                                            ui.label(
                                                RichText::new(&pm.pay_date)
                                                    .size(12.0)
                                                    .color(p.text_muted),
                                            );
                                        });
                                        row.col(|ui| {
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(
                                                            pm.amount_cent.format_yuan(),
                                                        )
                                                        .size(12.5)
                                                        .color(p.text),
                                                    );
                                                },
                                            );
                                        });
                                        row.col(|ui| {
                                            ui.label(
                                                RichText::new(
                                                    pm.reason.clone().unwrap_or_else(|| "—".into()),
                                                )
                                                .size(12.0)
                                                .color(p.text_muted),
                                            );
                                        });
                                        row.col(|ui| {
                                            if pm.status == ctp_core::payment::PaymentStatus::Paid
                                            {
                                                widgets::badge(ui, &p, "已付", Tone::Success);
                                            } else {
                                                widgets::badge(ui, &p, "作废", Tone::Neutral);
                                            }
                                        });
                                        row.col(|ui| {
                                            if pm.status == ctp_core::payment::PaymentStatus::Paid
                                                && ui
                                                    .add(
                                                        egui::Button::new(
                                                            RichText::new("作废")
                                                                .size(11.5)
                                                                .color(p.danger),
                                                        )
                                                        .frame(false),
                                                    )
                                                    .on_hover_text("作废后不计入已付净额，可重新登记")
                                                    .clicked()
                                            {
                                                void_target = Some((*pm).clone());
                                            }
                                        });
                                    });
                                }
                            });
                    });
                },
            );
        }
    });

    if submit {
        let amount = Money::parse_yuan(&app.pay_form.amount).unwrap_or(Money::ZERO);
        let date = app.pay_form.date.trim().to_string();
        let reason = {
            let r = app.pay_form.reason.trim().to_string();
            if r.is_empty() {
                "界面登记".to_string()
            } else {
                r
            }
        };
        let idx = app.pay_form.method_idx;
        let input = crate::dialogs::new_payment_input(&cid, amount, &date, &reason, idx);
        match app.exec(move |c| service::register_payment(c, input, "界面用户")) {
            Ok(pm) => {
                app.refresh();
                app.pay_form = crate::dialogs::PayForm::default();
                app.success(format!(
                    "已登记 {}，金额 {} 元",
                    pm.payment_no,
                    pm.amount_cent.format_yuan()
                ));
            }
            Err(e) => app.error(format!("登记被拒绝：{e}")),
        }
    }
    if let Some(pm) = void_target {
        app.confirm_reason.clear();
        app.confirm = Some(Confirm::VoidPayment(pm));
    }
}
