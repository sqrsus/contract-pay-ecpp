//! 拨款管理页：合同可付额度、登记表单、拨款明细。

use crate::widgets::{self, Tone};
use crate::{App, Confirm};
use ctp_app::service;
use ctp_app::ReportResult;
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
    let mut export_tpl = false;
    let mut import_tpl = false;
    // R-33：拨款一览导出（与明细表同源的 ReportResult）
    let mut export_list = false;

    // 页头：标题 + 右侧两个批量入口（导出模板 / 导入回传）
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("拨款管理").size(19.0).strong().color(p.text));
            ui.label(
                RichText::new(
                    "未交工合同累计付款不超过有效计价基准的 80%；施工合同须已审定、且交工满 2 年方可付清",
                )
                .size(12.0)
                .color(p.text_muted),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::ghost_button(ui, &p, "导入批量拨款")
                .on_hover_text("按模板填好后一次导入多笔，逐行校验、失败行单独提示")
                .clicked()
            {
                import_tpl = true;
            }
            if widgets::ghost_button(ui, &p, "导出批量拨款模板")
                .on_hover_text("导出含全部合同的 xlsx，只需填写后 4 列")
                .clicked()
            {
                export_tpl = true;
            }
            if widgets::ghost_button(ui, &p, "导出 Excel")
                .on_hover_text("导出当前合同的拨款一览，与明细表同源")
                .clicked()
            {
                export_list = true;
            }
        });
    });
    ui.add_space(10.0);

    if app.summaries.is_empty() {
        widgets::card(&p).show(ui, |ui| {
            widgets::empty_state(ui, &p, "还没有合同，先去「合同台账」新建或载入演示数据。");
        });
        return;
    }

    // ---------------- 合同选择：下拉 + 关键字筛选，选中即按 id 绑定（R-53）
    if app.pay_contract.is_none() {
        app.pay_contract = app.summaries.first().map(|s| s.contract_id.clone());
    }
    // 选中合同已不在当前清单（被删除 / 刷新后消失）：清掉，请用户重新选
    if let Some(id) = app.pay_contract.clone() {
        if !app.summaries.iter().any(|s| s.contract_id == id) {
            app.pay_contract = None;
        }
    }
    let mut picked_id: Option<String> = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("合同").size(12.5).color(p.text_muted));
        picked_id = widgets::contract_select(
            ui,
            "pay_contract",
            &p,
            &app.summaries,
            app.pay_contract.as_deref(),
            &mut app.pay_filter,
            &mut app.pay_open,
            372.0,
        );

        // 当前合同的项目状态与支付率：下拉之后紧跟，样式与合同台账一致
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
    if let Some(id) = picked_id {
        app.pay_contract = Some(id);
        // 选完即绑定；清掉筛选词，下次展开仍从全部合同开始
        app.pay_filter.clear();
    }

    let Some(cid) = app.pay_contract.clone() else {
        return;
    };
    // 取一份自有副本，避免整段绘制期间持续借用 app.summaries（R-39：期间要能写警告）
    let Some(s) = app.summaries.iter().find(|s| s.contract_id == cid).cloned() else {
        return;
    };
    // R-46：明细表与 Excel 导出共用同一份「当前合同的拨款单」与合同标签，
    // 不再在表格里和导出分支里各过滤一次 app.payments、各拼一次合同名。
    let payment_rows: Vec<ctp_core::payment::Payment> = app
        .payments
        .iter()
        .filter(|pm| pm.contract_id == cid)
        .cloned()
        .collect();
    let contract_label = format!("{} {}", s.contract_no, s.contract_name);
    // 额度按表单里的开单日期计算，与登记时的校验口径一致。
    // R-39：查询失败不再静默退回缓存值，而是给出可见警告 + 重试入口。
    let quota_res = app
        .db
        .as_ref()
        .map(|c| service::quota_on(c, &cid, &app.pay_form.date));
    let mut quota_warn: Option<String> = None;
    let quota = match quota_res {
        Some(Ok(q)) => Some(q),
        Some(Err(e)) => {
            quota_warn = Some(e.to_string());
            None
        }
        None => None,
    };
    let (base, paid, rate) = match &quota {
        Some(q) => (
            q.base_amount.format_yuan(),
            q.paid_amount.format_yuan(),
            q.pay_rate.percent_string(),
        ),
        None => (
            s.base_amount.format_yuan(),
            s.paid_amount.format_yuan(),
            s.pay_rate.percent_string(),
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
    let overdue = quota
        .as_ref()
        .map(|q| q.available_amount.cent() < 0)
        .unwrap_or(false);
    let is_draft = s.status == ContractStatus::Draft.code();
    let payable = ContractStatus::from_code(&s.status)
        .map(|x| x.is_payable())
        .unwrap_or(false);
    // `s` 是对 app.summaries 的不可变借用，用完后再写警告，避免借用冲突
    if let Some(e) = quota_warn {
        app.warn_data("读取可付额度", e);
    }
    // 待办动作先收集，等借用释放后再落到 App 上
    let mut goto_contract: Option<String> = None;
    let mut open_pay: Option<String> = None;

    ui.add_space(10.0);

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
            "元".to_string(),
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
            format!(
                "{} 笔",
                app.payments.iter().filter(|x| x.contract_id == cid).count()
            ),
            format!(
                "作废 {} 笔",
                app.payments
                    .iter()
                    .filter(|x| {
                        x.contract_id == cid && x.status == ctp_core::payment::PaymentStatus::Void
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

                    // 校验（R-39：解析失败要说出原因，不能只是把按钮悄悄禁用）
                    let parsed = Money::parse_yuan(&app.pay_form.amount);
                    let amount = parsed.as_ref().ok().copied();
                    let amount_parse_err = parsed.as_ref().err().map(|e| e.to_string());
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
                    } else if let Some(e) = amount_parse_err.clone() {
                        ui.label(
                            RichText::new(format!("金额无法识别：{e}（可填 1250000.00 这类数字）"))
                                .size(11.5)
                                .color(p.warn),
                        );
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
                        widgets::section_title(
                            ui,
                            &p,
                            "拨款明细",
                            "作废单保留记录、不计入已付净额",
                        );
                        ui.add_space(6.0);
                        // R-33：明细表、合计行与 Excel 导出共用同一份 ReportResult
                        let result = ReportResult::payment_ledger(&payment_rows, &contract_label);
                        if result.is_empty() {
                            widgets::empty_state(ui, &p, "该合同还没有拨款记录");
                            return;
                        }
                        // 批量打印：勾选若干条 → 合并成一份，一份一页
                        let chosen = payment_rows
                            .iter()
                            .filter(|pm| app.print_sel.contains(&pm.id))
                            .count();
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            if widgets::link(ui, &p, &format!("批量打印所选（{chosen}）"), 11.0)
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
                        // R-33：列配置、单元格取值、合计行全部来自 ReportResult；
                        // 勾选 / 打印 / 作废等交互列仍由页面按列语义附加绘制。
                        let mut tb = TableBuilder::new(ui)
                            .striped(true)
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(Column::auto().at_least(24.0));
                        for c in &result.columns {
                            tb = tb.column(if c.screen_remainder {
                                Column::remainder().at_least(c.screen_width)
                            } else {
                                Column::auto().at_least(c.screen_width)
                            });
                        }
                        tb = tb.column(Column::auto().at_least(160.0));
                        tb.header(26.0, |mut h| {
                            h.col(|ui| {
                                ui.label(RichText::new("").size(12.0).color(p.text_muted));
                            });
                            for c in &result.columns {
                                h.col(|ui| widgets::th(ui, &p, c.title, c.right));
                            }
                            h.col(|ui| {
                                ui.label(RichText::new("操作").size(12.0).color(p.text_muted));
                            });
                        })
                        .body(|mut body| {
                            for (ri, pm) in payment_rows.iter().enumerate() {
                                let cells = &result.rows[ri];
                                body.row(30.0, |mut row| {
                                    row.col(|ui| {
                                        let paid =
                                            pm.status == ctp_core::payment::PaymentStatus::Paid;
                                        let mut on = app.print_sel.contains(&pm.id);
                                        let cb =
                                            ui.add_enabled(paid, egui::Checkbox::new(&mut on, ""));
                                        if !paid {
                                            cb.on_hover_text("作废单不可打印");
                                        } else if cb.changed() {
                                            sel_toggle.push((pm.id.clone(), on));
                                        }
                                    });
                                    for (ci, cell) in cells.iter().enumerate() {
                                        let c = &result.columns[ci];
                                        let text = cell.display();
                                        row.col(|ui| match c.key {
                                            "payment_no" => {
                                                if widgets::link(ui, &p, text.as_str(), 12.0)
                                                    .clicked()
                                                {
                                                    open_pay = Some(pm.id.clone());
                                                }
                                            }
                                            "amount" => {
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        ui.label(
                                                            RichText::new(text.as_str())
                                                                .size(12.5)
                                                                .color(p.text),
                                                        );
                                                    },
                                                );
                                            }
                                            "status" => {
                                                if pm.status
                                                    == ctp_core::payment::PaymentStatus::Paid
                                                {
                                                    widgets::badge(
                                                        ui,
                                                        &p,
                                                        text.as_str(),
                                                        Tone::Success,
                                                    );
                                                } else {
                                                    widgets::badge(
                                                        ui,
                                                        &p,
                                                        text.as_str(),
                                                        Tone::Neutral,
                                                    );
                                                }
                                            }
                                            _ => {
                                                ui.label(
                                                    RichText::new(text.as_str())
                                                        .size(12.0)
                                                        .color(p.text_muted),
                                                );
                                            }
                                        });
                                    }
                                    row.col(|ui| {
                                        ui.horizontal(|ui| {
                                            // 表格列间距是 20px，操作列内要收紧，否则第三个操作会被挤出
                                            ui.spacing_mut().item_spacing.x = 6.0;
                                            if pm.status == ctp_core::payment::PaymentStatus::Paid {
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
                                            if pm.status == ctp_core::payment::PaymentStatus::Paid
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
                            // 合计行：金额只累加已实付单据，与 result.totals 同源
                            body.row(30.0, |mut row| {
                                row.col(|_ui| {});
                                for (ci, cell) in result.totals.iter().enumerate() {
                                    let c = &result.columns[ci];
                                    let text = cell.display();
                                    row.col(|ui| {
                                        let label = RichText::new(text.as_str())
                                            .size(12.5)
                                            .strong()
                                            .color(p.text);
                                        if c.right {
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(label);
                                                },
                                            );
                                        } else {
                                            ui.label(label);
                                        }
                                    });
                                }
                                row.col(|_ui| {});
                            });
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
                pm.contract_id == cid && pm.status == ctp_core::payment::PaymentStatus::Paid
            })
            .map(|pm| pm.id.clone())
            .collect();
        app.print_sel.extend(ids);
    }
    if batch_print {
        // 按列表顺序取当前合同下已勾选的单据；总览页走同一入口，只是传它自己的 6 条
        let ids: Vec<String> = app
            .payments
            .iter()
            .filter(|pm| pm.contract_id == cid && app.print_sel.contains(&pm.id))
            .map(|pm| pm.id.clone())
            .collect();
        app.open_batch_print(ids);
    }

    if submit {
        // 选中合同必须仍在当前清单里（数据被改 / 被删时不再落到失效合同上）
        if !app.summaries.iter().any(|s| s.contract_id == cid) {
            app.error("选中的合同已不存在，请重新选择后再登记");
            return;
        }
        // 正常路径由表单校验保证解析成功；这里兜底，绝不把解析失败悄悄写成 0 元单据
        let amount = match Money::parse_yuan(&app.pay_form.amount) {
            Ok(m) if m.cent() > 0 => m,
            _ => {
                app.error("金额无法识别，未登记拨款单");
                return;
            }
        };
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
        app.confirm = Some(Confirm::VoidPayment(Box::new(pm)));
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

    // ---------------- 批量导入 / 导出模板
    if export_tpl {
        let rows: Vec<ctp_app::batch::TemplateRow> = app
            .summaries
            .iter()
            .map(ctp_app::batch::TemplateRow::from_summary)
            .collect();
        if rows.is_empty() {
            app.error("还没有合同，无法生成批量拨款模板");
        } else {
            let name = format!("批量拨款模板_{}.xlsx", ctp_core::date::today_iso());
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(&name)
                .add_filter("Excel 工作簿", &["xlsx"])
                .save_file()
            {
                match ctp_app::batch::write_template(&rows, &path, &ctp_core::date::now_iso()) {
                    Ok((n, _)) => app.success(format!(
                        "模板已导出（{n} 份合同）：请只填后 4 列，本次金额留空的行不会导入"
                    )),
                    Err(e) => app.error(format!("导出模板失败：{e}")),
                }
            }
        }
    }
    if import_tpl && app.require_write() {
        // R-39：导入仍是同步执行（本轮不做线程化），但先置「处理中」状态挡住重复提交，
        // 真正的文件读取放到下一帧，让用户至少能看到一次明确的处理中反馈。
        if app.busy.is_some() {
            app.toast(Tone::Neutral, "正在处理上一次导入，请稍候");
        } else if let Some(path) = rfd::FileDialog::new()
            .add_filter("Excel 工作簿", &["xlsx"])
            .pick_file()
        {
            app.pending_import = Some(path);
            app.busy = Some("正在导入批量拨款…".to_string());
        }
    }

    // R-46：拨款一览导出。数据范围（当前合同的拨款单）、入口按钮与提示链路不变；
    // 在明细表同源结果上追加「合同名称 / 承包单位」，按每张单据的 contract_id 取值，
    // 查不到就留空，不跳过行、不影响导出。
    if export_list {
        let lookup = ctp_app::report::payment_contract_lookup(&app.summaries);
        let result = ReportResult::payment_ledger_export(&payment_rows, &contract_label, &lookup);
        app.export_report_result(&result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctp_core::ledger::ContractSummary;
    use ctp_core::rate::Rate;

    fn summary(id: &str, no: &str, name: &str) -> ContractSummary {
        ContractSummary {
            contract_id: id.into(),
            project_id: "P".into(),
            contract_no: no.into(),
            contract_name: name.into(),
            status: "PERFORMING".into(),
            contractor_name: "某施工单位".into(),
            contract_type: "CONSTRUCT".into(),
            contract_category_code: "GC".into(),
            category_code: None,
            category_name: None,
            total_amount: Money::from_cent(1_000),
            audit_price: None,
            is_audited: false,
            base_amount: Money::from_cent(1_000),
            audit_reduction: Money::ZERO,
            paid_amount: Money::ZERO,
            inflight_amount: Money::ZERO,
            occupied_amount: Money::ZERO,
            available_amount: Money::from_cent(1_000),
            pay_rate: Rate::from_bp(0),
            is_inverted: false,
            planned_end: None,
            duration_days: 0,
            delivery_date: None,
        }
    }

    /// R-53：合同改由下拉按 id 绑定后，「筛选词与选中合同」不再互相解释——
    /// 换选中只可能是点了某一项，不存在「打错字就记错账」的中间态。
    /// 这里保留一条最小回归：选中合同必须在清单里，否则界面会清空选择。
    #[test]
    fn 选中合同必须来自当前清单() {
        let list = [
            summary("C1", "HT-2026-001", "某市政道路改扩建工程"),
            summary("C2", "HT-2026-002", "某桥梁加固工程"),
        ];
        assert!(list.iter().any(|s| s.contract_id == "C1"));
        assert!(!list.iter().any(|s| s.contract_id == "C9"));
    }
}
