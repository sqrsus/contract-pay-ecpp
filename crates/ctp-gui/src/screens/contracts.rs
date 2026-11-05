//! 合同台账页：筛选、列表、右侧详情与操作入口。

use crate::screens::page_header;
use crate::widgets::{self, Tone};
use crate::App;
use ctp_app::service;
use ctp_core::contract::ContractStatus;
use ctp_core::money::Money;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

enum Act {
    /// 点击合同编号：打开只读详情弹窗
    DetailModal(String),
    /// 点击合同名称：选中并展开 / 收起拨款明细
    Expand(String),
    /// 展开区里点拨款单号：打开单据详情
    OpenPayment(String),
    New,
    Activate,
    Audit,
    /// 项目交工：登记交工日期
    Delivery,
    ToPayments(String),
    /// 打开附件管理弹窗
    Attachments,
}

const STATUSES: [ContractStatus; 2] = [ContractStatus::Performing, ContractStatus::Closed];

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;

    // 展开的拨款明细：点击名称/面板以外的区域自动收起（用上一帧的矩形判断）
    let keep_id = egui::Id::new("contract_expand_keep");
    let keep: Option<egui::Rect> = ui
        .memory(|m| m.data.get_temp::<Option<egui::Rect>>(keep_id))
        .flatten();
    if app.expanded_contract.is_some() && ui.input(|i| i.pointer.any_pressed()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            if keep.map_or(true, |r| !r.contains(pos)) {
                app.expanded_contract = None;
            }
        }
    }
    page_header(
        ui,
        &p,
        "合同台账",
        "点击编号查看详情；点击合同名称展开该合同的拨款明细",
    );

    let mut action: Option<Act> = None;
    // 展开面板里的打印入口：与拨款管理页「拨款明细」操作列一致
    let mut print_pay: Option<ctp_core::payment::Payment> = None;
    let mut print_material: Option<String> = None;

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
    let avail_w = ui.available_width();
    let detail_w = 300.0_f32;
    // 详情卡要占 300 + 12，窗口太窄时先保证列表不被挤压：不显示详情卡
    let show_detail = app.selected.is_some() && !app.narrow && avail_w > 1060.0;
    let table_w = if show_detail { avail_w - detail_w - 12.0 } else { avail_w };
    let page_h = ui.available_height();

    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(table_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                widgets::card(&p).show(ui, |ui| {
                    ui.set_width(table_w - 34.0);
                    let card_w = ui.available_width();
                    let kw = app.search.trim().to_lowercase();
                    let rows: Vec<&ctp_core::ledger::ContractSummary> = app
                        .summaries
                        .iter()
                        .filter(|s| {
                            let hit = kw.is_empty()
                                || s.contract_no.to_lowercase().contains(&kw)
                                || s.contract_name.to_lowercase().contains(&kw)
                                || s.contractor_name.to_lowercase().contains(&kw);
                            let st = match app.status_filter {
                                Some(ContractStatus::Performing) => s.pay_rate.bp() < 10_000,
                                Some(ContractStatus::Closed) => s.pay_rate.bp() >= 10_000,
                                _ => true,
                            };
                            hit && st
                        })
                        .collect();

                    if rows.is_empty() {
                        widgets::empty_state(ui, &p, "没有符合条件的合同");
                        return;
                    }

                    // 选中合同后右侧详情卡会把表格压窄：按「列宽需求」逐级降列，
                    // 避免固定列宽总和超出卡片，把详情卡挤出窗口右边界。
                    let c_no = 90.0;
                    let c_paid = 104.0;
                    let c_rate = 128.0;
                    let c_status = 72.0;
                    // 8 列（含承包单位/金额）836 / 7 列 714 / 5 列 514
                    const NEED_8: f32 = 684.0 + 120.0 + 8.0 * 4.0;
                    const NEED_7: f32 = 574.0 + 112.0 + 7.0 * 4.0;
                    let (compact, tight_cols) = if card_w >= NEED_8 {
                        (false, false)
                    } else if card_w >= NEED_7 {
                        (true, false)
                    } else {
                        (true, true)
                    };
                    let c_contractor = if compact { 0.0 } else { 110.0 };
                    let c_amount = if compact { 0.0 } else { 92.0 };
                    let c_audit = if tight_cols { 0.0 } else { 92.0 };
                    let c_delivery = if tight_cols { 0.0 } else { 88.0 };
                    let cell_count = if tight_cols {
                        5.0
                    } else if compact {
                        7.0
                    } else {
                        8.0
                    };
                    let name_min = if tight_cols {
                        100.0
                    } else if compact {
                        112.0
                    } else {
                        120.0
                    };
                    let fixed = c_no
                        + c_contractor
                        + c_amount
                        + c_audit
                        + c_delivery
                        + c_paid
                        + c_rate
                        + c_status;
                    let name_w = (card_w - fixed - cell_count * 4.0).max(name_min);

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        table_head(ui, &p, c_no, "合同编号", false);
                        table_head(ui, &p, name_w, "合同名称", false);
                        if !compact {
                            table_head(ui, &p, c_contractor, "承包单位", false);
                            table_head(ui, &p, c_amount, "合同金额", true);
                        }
                        if !tight_cols {
                            table_head(ui, &p, c_audit, "审计价", true);
                            table_head(ui, &p, c_delivery, "交工日期", false);
                        }
                        table_head(ui, &p, c_paid, "累计已付", true);
                        table_head(ui, &p, c_rate, "支付率", false);
                        table_head(ui, &p, c_status, "项目状态", false);
                    });
                    ui.add_space(2.0);

                    let mut detail: Option<String> = None;
                    let mut expand: Option<String> = None;
                    let mut open_pay: Option<String> = None;
                    let mut keep_rect: Option<egui::Rect> = None;

                    egui::ScrollArea::vertical()
                        .id_salt("contract_list_scroll")
                        .auto_shrink([false, false])
                        .max_height((page_h - 64.0).max(220.0))
                        .min_scrolled_height((page_h - 110.0).max(200.0))
                        .show(ui, |ui| {
                            for (idx, s) in rows.iter().enumerate() {
                                let selected = app.selected.as_deref() == Some(&s.contract_id);
                                let expanded =
                                    app.expanded_contract.as_deref() == Some(&s.contract_id);
                                let mut name_rect: Option<egui::Rect> = None;
                                let fill = if idx % 2 == 1 {
                                    p.surface_alt
                                } else {
                                    egui::Color32::TRANSPARENT
                                };
                                egui::Frame::none()
                                    .fill(fill)
                                    .inner_margin(egui::Margin::symmetric(0.0, 3.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 0.0;
                                            table_cell(ui, c_no, false, |ui| {
                                                let mut txt = RichText::new(&s.contract_no)
                                                    .size(12.5)
                                                    .color(p.primary);
                                                if selected {
                                                    txt = txt.strong().underline();
                                                }
                                                let r = ui
                                                    .add(
                                                        egui::Label::new(txt)
                                                            .sense(egui::Sense::click()),
                                                    )
                                                    .on_hover_cursor(
                                                        egui::CursorIcon::PointingHand,
                                                    )
                                                    .on_hover_text("查看合同详情");
                                                if r.clicked() {
                                                    detail = Some(s.contract_id.clone());
                                                }
                                            });
                                            table_cell(ui, name_w, false, |ui| {
                                                ui.label(
                                                    RichText::new(if expanded {
                                                        "－"
                                                    } else {
                                                        "＋"
                                                    })
                                                    .size(11.0)
                                                    .color(p.text_muted),
                                                );
                                                let r = widgets::clipped_text(
                                                    ui,
                                                    &s.contract_name,
                                                    (name_w - 22.0).max(20.0),
                                                    12.5,
                                                    if selected { p.primary } else { p.text },
                                                    egui::Sense::click(),
                                                )
                                                .on_hover_cursor(
                                                    egui::CursorIcon::PointingHand,
                                                )
                                                .on_hover_text(
                                                    "点击展开 / 收起该合同的拨款明细",
                                                );
                                                if r.clicked() {
                                                    expand = Some(s.contract_id.clone());
                                                }
                                                if expanded {
                                                    name_rect = Some(r.rect);
                                                }
                                            });
                                            if !compact {
                                                table_cell(ui, c_contractor, false, |ui| {
                                                    widgets::clipped_text(
                                                        ui,
                                                        &s.contractor_name,
                                                        (c_contractor - 8.0).max(20.0),
                                                        12.0,
                                                        p.text_muted,
                                                        egui::Sense::hover(),
                                                    );
                                                });
                                                table_cell(ui, c_amount, true, |ui| {
                                                    ui.label(
                                                        RichText::new(
                                                            s.total_amount.format_yuan(),
                                                        )
                                                        .size(12.5)
                                                        .color(p.text),
                                                    );
                                                });
                                            }
                                            if !tight_cols {
                                                table_cell(ui, c_audit, true, |ui| {
                                                    ui.label(
                                                        RichText::new(
                                                            s.audit_price
                                                                .map(|m| m.format_yuan())
                                                                .unwrap_or_else(|| "—".into()),
                                                        )
                                                        .size(12.5)
                                                        .color(if s.is_audited {
                                                            p.text
                                                        } else {
                                                            p.text_muted
                                                        }),
                                                    );
                                                });
                                                table_cell(ui, c_delivery, false, |ui| {
                                                    ui.label(
                                                        RichText::new(
                                                            s.delivery_date.as_deref().unwrap_or("—"),
                                                        )
                                                        .size(12.0)
                                                        .color(if s.delivery_date.is_some() {
                                                            p.text
                                                        } else {
                                                            p.text_muted
                                                        }),
                                                    );
                                                });
                                            }
                                            table_cell(ui, c_paid, true, |ui| {
                                                ui.label(
                                                    RichText::new(s.paid_amount.format_yuan())
                                                        .size(12.5)
                                                        .color(p.text),
                                                );
                                            });
                                            table_cell(ui, c_rate, false, |ui| {
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
                                            table_cell(ui, c_status, false, |ui| {
                                                let (label, tone) = project_status(s);
                                                widgets::badge(ui, &p, label, tone);
                                            });
                                        });
                                    });
                                if expanded {
                                    let panel = expand_panel(
                                        ui,
                                        &p,
                                        s,
                                        &app.payments,
                                        &mut open_pay,
                                        &mut print_pay,
                                        &mut print_material,
                                    );
                                    keep_rect = Some(name_rect.map_or(panel, |n| n.union(panel)));
                                }
                            }
                        });

                    ui.memory_mut(|m| m.data.insert_temp(keep_id, keep_rect));

                    if let Some(id) = expand {
                        action = Some(Act::Expand(id));
                    } else if let Some(id) = detail {
                        action = Some(Act::DetailModal(id));
                    }
                    if let Some(id) = open_pay {
                        action = Some(Act::OpenPayment(id));
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
        Some(Act::DetailModal(id)) => app.open_contract_detail(&id),
        Some(Act::Expand(id)) => {
            app.select(&id);
            app.toggle_expand(&id);
        }
        Some(Act::OpenPayment(id)) => app.open_payment(&id),
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
        Some(Act::Delivery) => {
            app.delivery_form = app
                .detail
                .as_ref()
                .and_then(|d| d.delivery_date.clone())
                .unwrap_or_else(ctp_core::date::today_iso);
            app.dialog = crate::dialogs::Dialog::DeliveryDate;
        }
        Some(Act::ToPayments(id)) => {
            app.pay_contract = Some(id);
            app.sync_pay_text();
            app.screen = crate::Screen::Payments;
        }
        None => {}
    }

    if let Some(pm) = print_pay {
        app.open_print_preview(&pm);
    }
    if let Some(cid) = print_material {
        app.open_material_print(&cid);
    }
}

/// 项目状态：支付率达 100% 自动「归档关闭」，否则「履行中」。
fn project_status(s: &ctp_core::ledger::ContractSummary) -> (&'static str, Tone) {
    if s.is_archived() {
        ("归档关闭", Tone::Neutral)
    } else {
        ("履行中", Tone::Success)
    }
}

/// 固定宽度的表格单元（表头与数据行共用，保证列对齐）。
fn table_cell(ui: &mut egui::Ui, width: f32, right: bool, add: impl FnOnce(&mut egui::Ui)) {
    let layout = if right {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    ui.allocate_ui_with_layout(egui::vec2(width, 26.0), layout, |ui| {
        ui.set_width(width);
        // 每列留 4px 内侧留白，避免相邻列文字贴在一起
        ui.add_space(4.0);
        add(ui);
    });
}

fn table_head(ui: &mut egui::Ui, p: &crate::theme::Palette, width: f32, text: &str, right: bool) {
    table_cell(ui, width, right, |ui| {
        ui.label(RichText::new(text).size(12.0).color(p.text_muted));
    });
}

/// 合同行下方展开的拨款明细面板：表头与「拨款管理 · 拨款明细」一致，已作废条目不再列出。
fn expand_panel(
    ui: &mut egui::Ui,
    p: &crate::theme::Palette,
    s: &ctp_core::ledger::ContractSummary,
    payments: &[ctp_core::payment::Payment],
    open_pay: &mut Option<String>,
    print_pay: &mut Option<ctp_core::payment::Payment>,
    print_material: &mut Option<String>,
) -> egui::Rect {
    let mut mine: Vec<&ctp_core::payment::Payment> = payments
        .iter()
        .filter(|pm| {
            pm.contract_id == s.contract_id
                && pm.status != ctp_core::payment::PaymentStatus::Void
        })
        .collect();
    mine.sort_by(|a, b| {
        b.pay_date
            .cmp(&a.pay_date)
            .then(b.payment_no.cmp(&a.payment_no))
    });

    let start = ui.min_rect();
    egui::Frame::none()
        .fill(p.primary_soft)
        .stroke(egui::Stroke::new(1.0_f32, p.border))
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(12.0, 9.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("拨款明细 · {} 笔", mine.len()))
                        .size(12.0)
                        .strong()
                        .color(p.text),
                );
                if !mine.is_empty() {
                    let net: i64 = mine.iter().map(|x| x.amount_cent.cent()).sum();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("净额 {} 元", Money::from_cent(net).format_yuan()))
                                .size(11.5)
                                .color(p.text_muted),
                        );
                    });
                }
            });
            ui.add_space(4.0);

            if mine.is_empty() {
                ui.label(RichText::new("该合同暂无有效拨款记录").size(11.5).color(p.text_muted));
                return;
            }

            // 列宽与拨款管理页保持同一套数值，读起来是同一张表
            ui.spacing_mut().item_spacing.x = 12.0;
            TableBuilder::new(ui)
                .striped(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::auto().at_least(104.0))
                .column(Column::auto().at_least(80.0))
                .column(Column::auto().at_least(92.0))
                .column(Column::remainder().at_least(60.0))
                .column(Column::auto().at_least(54.0))
                .column(Column::auto().at_least(150.0))
                .header(22.0, |mut h| {
                    for t in ["拨款单号", "开单日期", "金额", "事由", "状态", "操作"] {
                        h.col(|ui| {
                            ui.label(RichText::new(t).size(11.5).color(p.text_muted));
                        });
                    }
                })
                .body(|mut body| {
                    for pm in mine.iter().take(8) {
                        body.row(28.0, |mut row| {
                            row.col(|ui| {
                                if widgets::link(ui, p, &pm.payment_no, 11.5)
                                    .on_hover_text("查看拨款单详情")
                                    .clicked()
                                {
                                    *open_pay = Some(pm.id.clone());
                                }
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(&pm.pay_date).size(11.5).color(p.text_muted),
                                );
                            });
                            row.col(|ui| {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(pm.amount_cent.format_yuan())
                                                .size(12.0)
                                                .color(p.text),
                                        );
                                    },
                                );
                            });
                            row.col(|ui| {
                                let w = ui.available_width().max(24.0);
                                widgets::clipped_text(
                                    ui,
                                    pm.reason.as_deref().unwrap_or("—"),
                                    w,
                                    11.5,
                                    p.text_muted,
                                    egui::Sense::hover(),
                                );
                            });
                            row.col(|ui| {
                                widgets::badge(ui, p, "已付", Tone::Success);
                            });
                            row.col(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                if widgets::link(ui, p, "打印拨款单", 11.0)
                                    .on_hover_text("预览付款打印模板的 9 个合并域取值")
                                    .clicked()
                                {
                                    *print_pay = Some((*pm).clone());
                                }
                                if widgets::link(ui, p, "打印附件", 11.0)
                                    .on_hover_text("中标通知书、审计报告等材料打印")
                                    .clicked()
                                {
                                    *print_material = Some(pm.contract_id.clone());
                                }
                            });
                        });
                    }
                });

            if mine.len() > 8 {
                ui.add_space(2.0);
                ui.label(
                    RichText::new("仅显示最近 8 笔，完整流水见「拨款管理」")
                        .size(10.5)
                        .color(p.text_muted),
                );
            }
        });
    ui.add_space(3.0);
    start.union(ui.min_rect())
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
            let (pstatus, ptone) = project_status(s);
            widgets::badge(ui, &p, pstatus, ptone);
            // 标题行右侧放「设为生效 / 作废」：前者是草稿态主行动，
            // 后者是低频危险操作，都不占底部操作区，避免按钮换行把卡片撑高
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
        // 可付额度按「付清条件」封顶后的口径，与拨款管理页一致
        let quota_view = s.to_quota(ctp_core::rate::Rate::from_bp(app.tolerance_bp));
        let payable_now = Money::from_cent(quota_view.payable_amount.cent().max(0));
        widgets::kv(ui, &p, "可付额度", &format!("{} 元", payable_now.format_yuan()));
        // 支付率后带上封顶比例：80% 说明付清条件尚未满足（不额外占一行）
        let rate_text = if quota_view.limit_note.is_some() {
            format!(
                "{}（上限 {}%）",
                s.pay_rate.percent_string(),
                quota_view.cap_bp / 100
            )
        } else {
            s.pay_rate.percent_string()
        };
        widgets::kv(ui, &p, "支付率", &rate_text);
        if let Some(d) = d {
            if !tight {
                widgets::kv(ui, &p, "签订日期", &d.sign_date);
                widgets::kv(
                    ui,
                    &p,
                    "交工日期",
                    d.delivery_date.as_deref().unwrap_or("—"),
                );
                widgets::kv(ui, &p, "合同类型", d.contract_type.label());
                widgets::kv(
                    ui,
                    &p,
                    "招标方式",
                    d.bid_method.map(|b| b.label()).unwrap_or("—"),
                );
                widgets::kv(
                    ui,
                    &p,
                    "开标时间",
                    d.open_bid_date.as_deref().unwrap_or("—"),
                );
                widgets::kv(
                    ui,
                    &p,
                    "项目负责人",
                    d.project_leader.as_deref().unwrap_or("—"),
                );
                widgets::kv(
                    ui,
                    &p,
                    "联系方式",
                    d.contact_info.as_deref().unwrap_or("—"),
                );
                let agent_name = d
                    .bid_agent_id
                    .as_ref()
                    .and_then(|id| {
                        app.contractors
                            .iter()
                            .find(|c| &c.id == id)
                            .map(|c| c.contractor_name.clone())
                    })
                    .unwrap_or_else(|| "—".to_string());
                widgets::kv(ui, &p, "招标代理", &agent_name);
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

        ui.add_space(6.0);
        widgets::divider(ui, &p);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(4.0, 4.0);
            ui.spacing_mut().item_spacing.x = 2.0;
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
            if widgets::ghost_button_sm(ui, &p, "项目交工").clicked() {
                *action = Some(Act::Delivery);
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
