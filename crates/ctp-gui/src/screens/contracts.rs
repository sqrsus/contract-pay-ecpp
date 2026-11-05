//! 合同台账页：筛选、列表、右侧详情与操作入口。

use crate::screens::page_header;
use crate::widgets::{self, Tone};
use crate::{App, Confirm};
use ctp_app::service;
use ctp_core::contract::ContractStatus;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

enum Act {
    Select(String),
    New,
    Activate,
    Audit,
    Amount,
    Void,
    ToPayments(String),
    /// 打开附件管理弹窗
    Attachments,
}

const STATUSES: [ContractStatus; 6] = [
    ContractStatus::Draft,
    ContractStatus::Active,
    ContractStatus::Performing,
    ContractStatus::Completed,
    ContractStatus::Void,
    ContractStatus::Closed,
];

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    page_header(
        ui,
        &p,
        "合同台账",
        "点击编号查看详情；金额与审计价变更均写入留痕，可回溯",
    );

    let mut action: Option<Act> = None;

    // ---------------- 工具栏
    ui.horizontal(|ui| {
        ui.add_sized(
            [250.0, 30.0],
            egui::TextEdit::singleline(&mut app.search)
                .hint_text("搜索编号 / 名称 / 承包单位")
                .margin(egui::Margin::symmetric(8.0, 4.0)),
        );

        let cur = app
            .status_filter
            .map(|s| s.label().to_string())
            .unwrap_or_else(|| "全部状态".into());
        egui::ComboBox::from_id_salt("status_filter")
            .width(120.0)
            .selected_text(cur)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(app.status_filter.is_none(), "全部状态")
                    .clicked()
                {
                    app.status_filter = None;
                }
                for s in STATUSES {
                    let sel = app.status_filter == Some(s);
                    if ui.selectable_label(sel, s.label()).clicked() {
                        app.status_filter = Some(s);
                    }
                }
            });

        ui.label(
            RichText::new(format!(
                "共 {} 份 · 已付 {} 元",
                app.summaries.len(),
                ctp_core::money::Money::from_cent(
                    app.summaries.iter().map(|s| s.paid_amount.cent()).sum::<i64>()
                )
                .format_yuan()
            ))
            .size(11.5)
            .color(p.text_muted),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::primary_button(ui, &p, "新建合同", true).clicked() {
                action = Some(Act::New);
            }
        });
    });

    ui.add_space(8.0);

    // ---------------- 列表 + 详情
    let show_detail = app.selected.is_some() && !app.narrow;
    let avail_w = ui.available_width();
    let detail_w = 352.0_f32;
    let table_w = if show_detail { avail_w - detail_w - 12.0 } else { avail_w };
    let page_h = ui.available_height();

    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(table_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                widgets::card(&p).show(ui, |ui| {
                    ui.set_width(table_w - 34.0);
                    let kw = app.search.trim().to_lowercase();
                    let rows: Vec<&ctp_core::ledger::ContractSummary> = app
                        .summaries
                        .iter()
                        .filter(|s| {
                            let hit = kw.is_empty()
                                || s.contract_no.to_lowercase().contains(&kw)
                                || s.contract_name.to_lowercase().contains(&kw)
                                || s.contractor_name.to_lowercase().contains(&kw);
                            let st = app
                                .status_filter
                                .map(|x| s.status == x.code())
                                .unwrap_or(true);
                            hit && st
                        })
                        .collect();

                    if rows.is_empty() {
                        widgets::empty_state(ui, &p, "没有符合条件的合同");
                        return;
                    }
                    let mut click: Option<String> = None;
                    ui.spacing_mut().item_spacing.x = 20.0;
                    // 选中合同后右侧详情卡会把表格压窄，此时收起「承包单位 / 合同金额」两列，
                    // 保证「状态」不会被裁掉（这两项在详情卡里都有）
                    let compact = table_w - 34.0 < 780.0;
                    let mut tb = TableBuilder::new(ui)
                        .striped(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::auto().at_least(96.0))
                        .column(Column::remainder().at_least(128.0));
                    if !compact {
                        tb = tb
                            .column(Column::auto().at_least(118.0))
                            .column(Column::auto().at_least(92.0));
                    }
                    tb.column(Column::auto().at_least(92.0))
                        .column(Column::auto().at_least(132.0))
                        .column(Column::auto().at_least(72.0))
                        .header(26.0, |mut h| {
                            h.col(|ui| {
                                ui.label(RichText::new("合同编号").size(12.0).color(p.text_muted));
                            });
                            h.col(|ui| {
                                ui.label(RichText::new("合同名称").size(12.0).color(p.text_muted));
                            });
                            if !compact {
                                h.col(|ui| {
                                    ui.label(
                                        RichText::new("承包单位").size(12.0).color(p.text_muted),
                                    );
                                });
                                h.col(|ui| {
                                    ui.label(
                                        RichText::new("合同金额").size(12.0).color(p.text_muted),
                                    );
                                });
                            }
                            h.col(|ui| {
                                ui.label(RichText::new("累计已付").size(12.0).color(p.text_muted));
                            });
                            h.col(|ui| {
                                ui.label(RichText::new("支付率").size(12.0).color(p.text_muted));
                            });
                            h.col(|ui| {
                                ui.label(RichText::new("状态").size(12.0).color(p.text_muted));
                            });
                        })
                        .body(|mut body| {
                            for s in &rows {
                                let selected = app.selected.as_deref() == Some(&s.contract_id);
                                body.row(30.0, |mut row| {
                                    row.col(|ui| {
                                        // 选中行的编号加粗下划线，配合右侧详情卡给出选中反馈
                                        let mut txt = RichText::new(&s.contract_no)
                                            .size(12.5)
                                            .color(p.primary);
                                        if selected {
                                            txt = txt.strong().underline();
                                        }
                                        if ui
                                            .add(egui::Label::new(txt).sense(egui::Sense::click()))
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            click = Some(s.contract_id.clone());
                                        }
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(&s.contract_name).size(12.5).color(p.text));
                                    });
                                    if !compact {
                                        row.col(|ui| {
                                            ui.label(
                                                RichText::new(&s.contractor_name)
                                                    .size(12.0)
                                                    .color(p.text_muted),
                                            );
                                        });
                                        row.col(|ui| {
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(s.total_amount.format_yuan())
                                                            .size(12.5)
                                                            .color(p.text),
                                                    );
                                                },
                                            );
                                        });
                                    }
                                    row.col(|ui| {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(s.paid_amount.format_yuan())
                                                        .size(12.5)
                                                        .color(p.text),
                                                );
                                            },
                                        );
                                    });
                                    row.col(|ui| {
                                        let frac = s.pay_rate.bp() as f32 / 10_000.0;
                                        let color = if s.is_inverted {
                                            p.danger
                                        } else if frac > 0.99 {
                                            p.warn
                                        } else {
                                            p.primary
                                        };
                                        widgets::bar(ui, &p, frac, color, 82.0, 7.0);
                                        ui.label(
                                            RichText::new(s.pay_rate.percent_string())
                                                .size(11.5)
                                                .color(p.text_muted),
                                        );
                                    });
                                    row.col(|ui| {
                                        let tone = match s.status.as_str() {
                                            "VOID" => Tone::Neutral,
                                            "DRAFT" => Tone::Warn,
                                            "COMPLETED" | "SETTLED" | "CLOSED" => Tone::Primary,
                                            _ => Tone::Success,
                                        };
                                        let label = ContractStatus::from_code(&s.status)
                                            .map(|x| x.label())
                                            .unwrap_or(&s.status);
                                        widgets::badge(ui, &p, label, tone);
                                    });
                                });
                            }
                        });
                    if let Some(id) = click {
                        action = Some(Act::Select(id));
                    }
                });
            },
        );

        if show_detail {
            ui.add_space(12.0);
            let max_h = (page_h - 8.0).max(260.0);
            ui.allocate_ui_with_layout(
                egui::vec2(detail_w, max_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    // 窗口高度不足时整卡可滚动，避免底部操作按钮被状态栏压住
                    egui::ScrollArea::vertical()
                        .id_salt("contract_detail_scroll")
                        .max_height(max_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_width(detail_w - 2.0);
                            detail_card(app, ui, &mut action, max_h);
                        });
                },
            );
        }
    });

    match action {
        Some(Act::Select(id)) => app.select(&id),
        Some(Act::New) => {
            app.new_contract = crate::dialogs::ContractForm::default();
            app.dialog = crate::dialogs::Dialog::NewContract;
        }
        Some(Act::Activate) => {
            if let Some(id) = app.selected.clone() {
                match app.exec(|conn| service::activate_contract(conn, &id, "界面用户")) {
                    Ok(c) => {
                        app.refresh();
                        app.select(&c.id);
                        app.success(format!("合同 {} 已生效，可以登记拨款", c.contract_no));
                    }
                    Err(e) => app.error(format!("生效失败：{e}")),
                }
            }
        }
        Some(Act::Attachments) => {
            if let Some(id) = app.selected.clone() {
                app.open_attachments(&id);
            }
        }
        Some(Act::Audit) => {
            if let Some(s) = app.selected_summary() {
                app.audit_form.amount = if s.is_audited {
                    s.audit_price.map(|m| m.format_yuan()).unwrap_or_default()
                } else {
                    String::new()
                };
            }
            app.dialog = crate::dialogs::Dialog::AuditPrice;
        }
        Some(Act::Amount) => {
            if let Some(s) = app.selected_summary() {
                app.amount_form.amount = s.total_amount.format_yuan();
                app.amount_form.reason.clear();
            }
            app.dialog = crate::dialogs::Dialog::EditAmount;
        }
        Some(Act::Void) => {
            if let Some(d) = app.detail.clone() {
                app.confirm = Some(Confirm::VoidContract(d));
            }
        }
        Some(Act::ToPayments(id)) => {
            app.pay_contract = Some(id);
            app.screen = crate::Screen::Payments;
        }
        None => {}
    }
}

fn detail_card(app: &App, ui: &mut egui::Ui, action: &mut Option<Act>, max_h: f32) {
    let p = app.pal;
    let Some(s) = app.selected_summary() else {
        return;
    };
    let d = app.detail.as_ref();
    let is_draft = s.status == ContractStatus::Draft.code();
    let can_pay = ContractStatus::from_code(&s.status)
        .map(|x| x.is_payable())
        .unwrap_or(false);
    // 窗口较矮时收起次要字段与最近拨款，保证底部操作按钮不会被窗口下沿裁掉
    let tight = max_h < 620.0;
    // 字段行较多，行距收紧到 7px，把内容压在卡片高度内
    ui.spacing_mut().item_spacing.y = 7.0;

    widgets::card(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(&s.contract_no).size(15.0).strong().color(p.text));
            let tone = if s.is_inverted { Tone::Danger } else { Tone::Success };
            widgets::badge(
                ui,
                &p,
                &ContractStatus::from_code(&s.status)
                    .map(|x| x.label().to_string())
                    .unwrap_or_else(|| s.status.clone()),
                tone,
            );
            // 标题行右侧放「设为生效 / 作废」：前者是草稿态主行动，
            // 后者是低频危险操作，都不占底部操作区，避免按钮换行把卡片撑高
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if s.status != "VOID"
                    && widgets::danger_button_sm(ui, &p, "作废").clicked()
                {
                    *action = Some(Act::Void);
                }
                if is_draft && widgets::primary_button_sm(ui, &p, "设为生效").clicked() {
                    *action = Some(Act::Activate);
                }
            });
        });
        ui.label(RichText::new(&s.contract_name).size(12.5).color(p.text_muted));
        ui.add_space(8.0);
        widgets::divider(ui, &p);

        widgets::kv(ui, &p, "承包单位", &s.contractor_name);
        widgets::kv(ui, &p, "合同金额", &format!("{} 元", s.total_amount.format_yuan()));
        widgets::kv(
            ui,
            &p,
            "审计价",
            &s.audit_price
                .map(|m| format!("{} 元", m.format_yuan()))
                .unwrap_or_else(|| "未审定".into()),
        );
        widgets::kv(ui, &p, "有效计价基准", &format!("{} 元", s.base_amount.format_yuan()));
        widgets::kv(ui, &p, "累计已付", &format!("{} 元", s.paid_amount.format_yuan()));
        widgets::kv(ui, &p, "可付额度", &format!("{} 元", s.available_amount.format_yuan()));
        widgets::kv(ui, &p, "支付率", &s.pay_rate.percent_string());
        if let Some(d) = d {
            if !tight {
                widgets::kv(ui, &p, "签订日期", &d.sign_date);
                widgets::kv(ui, &p, "合同类型", d.contract_type.label());
                if let Some(cat) = &s.category_name {
                    widgets::kv(ui, &p, "会计分类", cat);
                }
            }
        }

        if s.is_inverted {
            ui.add_space(6.0);
            widgets::soft_box(&p, p.danger_soft).show(ui, |ui| {
                ui.label(
                    RichText::new("已超有效计价基准：累计已付高于基准，请核对审计价或拨款单。")
                        .size(11.5)
                        .color(p.danger),
                );
            });
        }

        // 该合同最近拨款（完整流水在「拨款管理」页）
        let mine: Vec<_> = app
            .payments
            .iter()
            .filter(|pm| pm.contract_id == s.contract_id)
            .collect();
        if !mine.is_empty() {
            ui.add_space(8.0);
            widgets::divider(ui, &p);
            ui.label(RichText::new("最近拨款").size(12.5).strong().color(p.text));
            ui.add_space(4.0);
            for pm in mine.iter().rev().take(3) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&pm.pay_date).size(11.5).color(p.text_muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if pm.status == ctp_core::payment::PaymentStatus::Void {
                            widgets::badge(ui, &p, "作废", Tone::Neutral);
                        }
                        ui.label(
                            RichText::new(pm.amount_cent.format_yuan())
                                .size(12.0)
                                .color(p.text),
                        );
                    });
                });
            }
        }

        ui.add_space(6.0);
        widgets::divider(ui, &p);
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
            let att_label = format!("附件 {}/4", app.att_count.clamp(0, 4));
            let att_btn = widgets::ghost_button_sm(ui, &p, &att_label);
            let att_btn = if app.att_count == 0 {
                att_btn.on_hover_text("四类附件均未上传；合同正本为必传项")
            } else {
                att_btn.on_hover_text("查看 / 上传 / 替换 / 另存附件")
            };
            if att_btn.clicked() {
                *action = Some(Act::Attachments);
            }
            if widgets::ghost_button_sm(ui, &p, "登记审计价").clicked() {
                *action = Some(Act::Audit);
            }
            if widgets::ghost_button_sm(ui, &p, "修改金额").clicked() {
                *action = Some(Act::Amount);
            }
            let pay = widgets::ghost_button_sm_on(ui, &p, "去拨款", can_pay);
            let pay = if can_pay {
                pay
            } else {
                pay.on_hover_text("草稿 / 作废合同不可拨款，先点标题行的「设为生效」")
            };
            if pay.clicked() && can_pay {
                *action = Some(Act::ToPayments(s.contract_id.clone()));
            }
        });
        if is_draft {
            ui.add_space(2.0);
            ui.label(
                RichText::new("草稿不可登记拨款；点标题行「设为生效」即可，生效要求合同附件已在位。")
                    .size(10.5)
                    .color(p.text_muted),
            );
        }
    });
}
