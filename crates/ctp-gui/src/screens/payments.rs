//! 拨款管理页：合同可付额度、登记表单、拨款明细。

use crate::screens::page_header;
use crate::widgets::{self, Tone};
use crate::{App, Confirm};
use ctp_app::service;
use ctp_core::contract::ContractStatus;
use ctp_core::date::is_iso_date;
use ctp_core::money::Money;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

/// 合同状态的显示名（未知代码原样返回）。
fn status_label(status: &str) -> &str {
    ContractStatus::from_code(status)
        .map(|x| x.label())
        .unwrap_or(status)
}

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    page_header(
        ui,
        &p,
        "拨款管理",
        "未交工合同累计付款不超过有效计价基准的 80%；施工合同须已审定、且交工满 2 年方可付清",
    );

    if app.summaries.is_empty() {
        widgets::card(&p).show(ui, |ui| {
            widgets::empty_state(ui, &p, "还没有合同，先去「合同台账」新建或载入演示数据。");
        });
        return;
    }

    // ---------------- 合同选择：输入即筛查，聚焦不全量展开
    if app.pay_contract.is_none() {
        app.pay_contract = app.summaries.first().map(|s| s.contract_id.clone());
        app.sync_pay_text();
        app.pay_contract_text = app.pay_contract_label().unwrap_or_default();
    }
    let candidates: Vec<String> = app
        .summaries
        .iter()
        .map(|s| format!("{} {}", s.contract_no, s.contract_name))
        .collect();
    let mut text = app.pay_contract_text.clone();
    let mut picked: Option<String> = None;
    let mut edited = false;

    ui.horizontal(|ui| {
        ui.label(RichText::new("合同").size(12.5).color(p.text_muted));
        let out = widgets::autocomplete_full(
            ui,
            "pay_contract",
            &p,
            &mut text,
            &candidates,
            372.0,
            "输入合同编号或名称后联想",
            widgets::SuggestMode::TypedOnly,
        );
        picked = out.picked;
        edited = out.changed;

        // 当前合同的项目状态与支付率：输入框之后紧跟，样式与合同台账一致
        if let Some(s) = app
            .pay_contract
            .as_ref()
            .and_then(|id| app.summaries.iter().find(|s| &s.contract_id == id))
        {
            ui.add_space(8.0);
            if s.is_archived() {
                widgets::badge(ui, &p, "归档关闭", Tone::Danger);
            } else {
                widgets::badge(ui, &p, "履行中", Tone::Success);
            }
            ui.add_space(8.0);
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
        }
    });

    app.pay_contract_text = text;
    if let Some(v) = picked {
        // 点选候选：按展示串反查合同 id
        if let Some(s) = app
            .summaries
            .iter()
            .find(|s| format!("{} {}", s.contract_no, s.contract_name) == v)
        {
            app.pay_contract = Some(s.contract_id.clone());
        }
    } else if edited {
        // 边输边匹配：唯一命中才切换，匹配不到沿用原合同
        if let Some(id) = unique_contract_match(&app.summaries, &app.pay_contract_text) {
            app.pay_contract = Some(id);
        }
    }

    let Some(cid) = app.pay_contract.clone() else {
        return;
    };
    let Some(s) = app.summaries.iter().find(|s| s.contract_id == cid) else {
        return;
    };
    // 额度按表单里的开单日期计算，与登记时的校验口径一致
    let quota = app
        .db
        .as_ref()
        .and_then(|c| service::quota_on(c, &cid, &app.pay_form.date).ok());
    let (base, paid, rate, tol_bp) = match &quota {
        Some(q) => (
            q.base_amount.format_yuan(),
            q.paid_amount.format_yuan(),
            q.pay_rate.percent_string(),
            q.tolerance.bp(),
        ),
        None => (
            s.base_amount.format_yuan(),
            s.paid_amount.format_yuan(),
            s.pay_rate.percent_string(),
            app.tolerance_bp,
        ),
    };
    // 可付额度取「付清条件」封顶后的口径
    let payable_cent = quota
        .as_ref()
        .map(|q| q.payable_amount.cent())
        .unwrap_or_else(|| s.available_amount.cent());
    let limit_cent = quota.as_ref().map(|q| q.limit_amount.cent()).unwrap_or(0);
    let cap_bp = quota.as_ref().map(|q| q.cap_bp).unwrap_or(10_000);
    let limit_note = quota.as_ref().and_then(|q| q.limit_note);
    let avail = Money::from_cent(payable_cent.max(0)).format_yuan();
    let overdue = quota.as_ref().map(|q| q.available_amount.cent() < 0).unwrap_or(false);
    let is_draft = s.status == ContractStatus::Draft.code();
    let payable = ContractStatus::from_code(&s.status)
        .map(|x| x.is_payable())
        .unwrap_or(false);
    // 待办动作先收集，等借用释放后再落到 App 上
    let mut goto_contract: Option<String> = None;
    let mut open_pay: Option<String> = None;

    ui.add_space(10.0);

    // ---------------- 付清条件提示
    if let Some(note) = limit_note {
        widgets::soft_box(&p, p.warn_soft).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(note).size(12.0).strong().color(p.warn));
                ui.label(
                    RichText::new(format!(
                        "｜当前累计上限 {}%（{} 元），已付 {} 元。",
                        cap_bp / 100,
                        Money::from_cent(limit_cent).format_yuan(),
                        paid
                    ))
                    .size(11.5)
                    .color(p.text_muted),
                );
            });
        });
        ui.add_space(10.0);
    }

    // ---------------- 不可付提示
    if !payable {
        widgets::soft_box(&p, p.warn_soft).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "该合同当前为「{}」，不可登记拨款。",
                        status_label(&s.status)
                    ))
                    .size(12.5)
                    .strong()
                    .color(p.warn),
                );
                if is_draft {
                    if widgets::link(ui, &p, "去合同台账设为生效 →", 12.5).clicked() {
                        goto_contract = Some(cid.clone());
                    }
                } else {
                    ui.label(
                        RichText::new("作废合同不再接受新的拨款单。")
                            .size(12.0)
                            .color(p.text_muted),
                    );
                }
            });
        });
        ui.add_space(10.0);
    }

    // ---------------- 额度卡片
    let avail_w = ui.available_width();
    let gap = 12.0_f32;
    let cols = (((avail_w + gap) / 252.0).floor() as usize).clamp(1, 4);
    let w = (avail_w - gap * (cols as f32 - 1.0)) / cols as f32;
    let cards: Vec<(&str, String, String, Tone)> = vec![
        (
            "有效计价基准",
            base.clone(),
            format!("元 · 容差 {tol_bp} bp"),
            Tone::Neutral,
        ),
        (
            "累计已付",
            paid.clone(),
            format!("元 · 已付率 {rate}"),
            Tone::Primary,
        ),
        (
            "可付额度",
            avail.clone(),
            if limit_note.is_some() {
                format!("元 · {}% 封顶", cap_bp / 100)
            } else if overdue {
                "元 · 已超额，需先处理".to_string()
            } else {
                "元 · 上限 − 已付".to_string()
            },
            if overdue { Tone::Danger } else { Tone::Success },
        ),
        (
            "已登记单据",
            format!("{} 笔", app.payments.iter().filter(|x| x.contract_id == cid).count()),
            format!(
                "作废 {} 笔",
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
    let mut print_target: Option<ctp_core::payment::Payment> = None;
    let mut material_target: Option<String> = None;
    // 批量打印：勾选状态与批量动作分开收集，遍历列表时不能改 App
    let mut sel_toggle: Vec<(String, bool)> = Vec::new();
    let mut batch_print = false;
    let mut select_all = false;
    let mut clear_sel = false;

    ui.horizontal_top(|ui| {
        let form_w = if avail_w > 900.0 { 300.0 } else { avail_w };
        ui.allocate_ui_with_layout(
            egui::vec2(form_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                widgets::card(&p).show(ui, |ui| {
                    ui.set_width(form_w - 34.0);
                    widgets::section_title(ui, &p, "登记拨款单", "单级登记，无需审批");
                    ui.add_space(6.0);

                    widgets::field_label(ui, &p, "* 本次金额（元）");
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [form_w - 40.0 - 56.0, 30.0],
                            egui::TextEdit::singleline(&mut app.pay_form.amount)
                                .hint_text("例如 1250000.00")
                                .margin(egui::Margin::symmetric(8.0, 4.0)),
                        );
                        // 付清 = 填入当前可付额度（已叠加付清条件封顶）
                        let fill = widgets::ghost_button_sm_on(ui, &p, "付清", payable_cent > 0);
                        let fill = if payable_cent > 0 {
                            fill.on_hover_text("填入当前可付额度")
                        } else {
                            fill.on_hover_text("当前无可付额度")
                        };
                        if fill.clicked() && payable_cent > 0 {
                            app.pay_form.amount = Money::from_cent(payable_cent).format_yuan();
                        }
                    });

                    ui.add_space(6.0);
                    widgets::field_label(ui, &p, "* 开单日期");
                    widgets::date_input(ui, "pay_date", &p, &mut app.pay_form.date, form_w - 40.0);

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
                    let over = amount.map(|m| m.cent() > payable_cent).unwrap_or(false);
                    let ok = amount_ok && date_ok && !over && payable;

                    ui.add_space(9.0);
                    if over {
                        widgets::soft_box(&p, p.warn_soft).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(format!(
                                    "超出可付额度：本次 {} 元 > 可付 {} 元",
                                    amount.map(|m| m.format_yuan()).unwrap_or_default(),
                                    avail
                                ))
                                .size(11.5)
                                .color(p.warn),
                            );
                            if limit_note.is_some() {
                                ui.label(
                                    RichText::new(format!(
                                        "原因：累计付款上限为 {}%（付清条件未满足）",
                                        cap_bp / 100
                                    ))
                                    .size(11.0)
                                    .color(p.text_muted),
                                );
                            }
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
                    if !payable {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("当前合同不可拨款，按钮已禁用")
                                .size(11.0)
                                .color(p.text_muted),
                        );
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
                        // 批量打印：勾选若干条 → 合并成一份，一份一页
                        let chosen = rows.iter().filter(|pm| app.print_sel.contains(&pm.id)).count();
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            if widgets::link(
                                ui,
                                &p,
                                &format!("批量打印所选（{chosen}）"),
                                11.0,
                            )
                            .on_hover_text("勾选的拨款单合并成一份 Word，每张一页")
                            .clicked()
                            {
                                batch_print = true;
                            }
                            if widgets::link(ui, &p, "全选", 11.0).clicked() {
                                select_all = true;
                            }
                            if chosen > 0 && widgets::link(ui, &p, "清空", 11.0).clicked() {
                                clear_sel = true;
                            }
                            ui.label(
                                RichText::new("作废单不参与勾选，也不可打印")
                                    .size(11.0)
                                    .color(p.text_muted),
                            );
                        });
                        ui.add_space(4.0);
                        ui.spacing_mut().item_spacing.x = 20.0;
                        TableBuilder::new(ui)
                            .striped(true)
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(Column::auto().at_least(24.0))
                            .column(Column::auto().at_least(104.0))
                            .column(Column::auto().at_least(80.0))
                            .column(Column::auto().at_least(92.0))
                            .column(Column::remainder().at_least(60.0))
                            .column(Column::auto().at_least(54.0))
                            .column(Column::auto().at_least(160.0))
                            .header(26.0, |mut h| {
                                for t in ["", "拨款单号", "开单日期", "金额", "事由", "状态", "操作"] {
                                    h.col(|ui| {
                                        ui.label(RichText::new(t).size(12.0).color(p.text_muted));
                                    });
                                }
                            })
                            .body(|mut body| {
                                for pm in rows.iter().copied() {
                                    body.row(30.0, |mut row| {
                                        row.col(|ui| {
                                            let paid = pm.status
                                                == ctp_core::payment::PaymentStatus::Paid;
                                            let mut on = app.print_sel.contains(&pm.id);
                                            let cb = ui.add_enabled(
                                                paid,
                                                egui::Checkbox::new(&mut on, ""),
                                            );
                                            if !paid {
                                                cb.on_hover_text("作废单不可打印");
                                            } else if cb.changed() {
                                                sel_toggle.push((pm.id.clone(), on));
                                            }
                                        });
                                        row.col(|ui| {
                                            if widgets::link(ui, &p, &pm.payment_no, 12.0).clicked()
                                            {
                                                open_pay = Some(pm.id.clone());
                                            }
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
                                            ui.horizontal(|ui| {
                                                // 表格列间距是 20px，操作列内要收紧，否则第三个操作会被挤出
                                                ui.spacing_mut().item_spacing.x = 6.0;
                                                if pm.status
                                                    == ctp_core::payment::PaymentStatus::Paid
                                                {
                                                    if widgets::link(ui, &p, "打印拨款单", 11.0)
                                                        .on_hover_text(
                                                            "预览两页版面后直接送打印机（不经过 Word）",
                                                        )
                                                        .clicked()
                                                    {
                                                        print_target = Some((*pm).clone());
                                                    }
                                                } else {
                                                    ui.label(
                                                        RichText::new("打印拨款单")
                                                            .size(11.0)
                                                            .color(p.text_muted),
                                                    )
                                                    .on_hover_text("作废单不允许打印");
                                                }
                                                if widgets::link(ui, &p, "打印附件", 11.0)
                                                    .on_hover_text("中标通知书、审计报告等材料打印")
                                                    .clicked()
                                                {
                                                    material_target = Some(pm.contract_id.clone());
                                                }
                                                if pm.status
                                                    == ctp_core::payment::PaymentStatus::Paid
                                                    && ui
                                                        .add(
                                                            egui::Button::new(
                                                                RichText::new("作废")
                                                                    .size(11.0)
                                                                    .color(p.danger),
                                                            )
                                                            .frame(false),
                                                        )
                                                        .on_hover_text(
                                                            "作废后不计入已付净额，可重新登记",
                                                        )
                                                        .clicked()
                                                {
                                                    void_target = Some((*pm).clone());
                                                }
                                            });
                                        });
                                    });
                                }
                            });
                    });
                },
            );
        }
    });

    // 勾选状态落回 App：表格遍历期间借用了 app.payments，不能就地改
    for (id, on) in sel_toggle {
        if on {
            app.print_sel.insert(id);
        } else {
            app.print_sel.remove(&id);
        }
    }
    if clear_sel {
        app.print_sel.clear();
    }
    if select_all {
        let ids: Vec<String> = app
            .payments
            .iter()
            .filter(|pm| {
                pm.contract_id == cid
                    && pm.status == ctp_core::payment::PaymentStatus::Paid
            })
            .map(|pm| pm.id.clone())
            .collect();
        app.print_sel.extend(ids);
    }
    if batch_print {
        app.open_batch_print();
    }

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
    if let Some(pm) = print_target {
        app.open_print_preview(&pm);
    }
    if let Some(cid) = material_target {
        app.open_material_print(&cid);
    }
    if let Some(id) = goto_contract {
        app.goto_contract(&id);
        return;
    }
    if let Some(id) = open_pay {
        app.open_payment(&id);
    }
}

/// 按输入文本找合同：整串「编号 名称」/ 编号 / 名称精确命中，或**唯一**包含命中。
///
/// 命中多于一份时返回 `None`——继续沿用用户原先选中的合同，避免边打字边跳合同。
fn unique_contract_match(
    summaries: &[ctp_core::ledger::ContractSummary],
    text: &str,
) -> Option<String> {
    let q = text.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let exact: Vec<&ctp_core::ledger::ContractSummary> = summaries
        .iter()
        .filter(|s| {
            format!("{} {}", s.contract_no, s.contract_name).to_lowercase() == q
                || s.contract_no.to_lowercase() == q
                || s.contract_name.to_lowercase() == q
        })
        .collect();
    if exact.len() == 1 {
        return Some(exact[0].contract_id.clone());
    }
    let hits: Vec<&ctp_core::ledger::ContractSummary> = summaries
        .iter()
        .filter(|s| {
            s.contract_no.to_lowercase().contains(&q) || s.contract_name.to_lowercase().contains(&q)
        })
        .collect();
    if hits.len() == 1 {
        Some(hits[0].contract_id.clone())
    } else {
        None
    }
}
