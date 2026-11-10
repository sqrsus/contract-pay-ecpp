//! 总览页：关键指标、风险提示、最近拨款。

use crate::widgets::{self, Tone};
use crate::App;
use ctp_core::money::Money;
use ctp_core::rate::Rate;
use eframe::egui::{self, RichText};

/// 最近拨款一行：(单 ID, 开单日期, 单号, 合同 ID, 合同名称, 承包单位, 金额分, 是否作废)
type RecentRow = (String, String, String, String, String, String, i64, bool);

/// 「最近拨款」合同名称列宽：R-56 加宽到 1.3 倍（236 → 306.8），
/// R-58 再按需求 ×1.2 → 368.16。表头、单元格与滚动宽度三处必须同值，否则文字与列边界会错位。
const NAME_W: f32 = 368.16;

/// 「最近拨款」承包单位列宽：130 × 1.2 = 156（R-58）。同样三处同值。
const CONTRACTOR_W: f32 = 156.0;

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;

    let mut new_contract = false;
    let mut new_payment = false;

    // 标题 + 右上快捷入口（与合同台账的「新建合同」保持同一视觉层级）
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("总览").size(19.0).strong().color(p.text));
            ui.label(
                RichText::new(
                    "已付 = 已实付单据净额（作废单不计）；有效基准 = 审计价 > 0 ? 审计价 : 合同金额",
                )
                .size(12.0)
                .color(p.text_muted),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::primary_button(ui, &p, "新建合同", true).clicked() {
                new_contract = true;
            }
            if widgets::ghost_button(ui, &p, "新建拨款").clicked() {
                new_payment = true;
            }
        });
    });
    ui.add_space(4.0);

    if app.summaries.is_empty() {
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("还没有合同数据")
                    .size(16.0)
                    .strong()
                    .color(p.text),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "从「合同台账」新建第一份合同；多份合同可先导出批量合同模板，填好后再导入。",
                )
                .size(12.5)
                .color(p.text_muted),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::ghost_button(ui, &p, "新建合同").clicked() && app.require_write() {
                    app.dialog = crate::dialogs::Dialog::NewContract;
                }
            });
        });
        if new_contract && app.require_write() {
            app.dialog = crate::dialogs::Dialog::NewContract;
        }
        if new_payment && app.require_write() {
            app.start_payment();
        }
        return;
    }

    let count = app.summaries.len();
    let archived = app.summaries.iter().filter(|s| s.is_archived()).count();
    let performing = count - archived;
    let total: i64 = Money::sum_saturating(app.summaries.iter().map(|s| s.total_amount.cent()));
    let base: i64 = Money::sum_saturating(app.summaries.iter().map(|s| s.base_amount.cent()));
    let paid: i64 = Money::sum_saturating(app.summaries.iter().map(|s| s.paid_amount.cent()));
    let risk = app.summaries.iter().filter(|s| s.is_inverted).count();
    let rate = Rate::ratio(Money::from_cent(paid), Money::from_cent(base.max(1)));

    // KPI 卡片：按可用宽度自适应列数（最多 4 列，8 张卡正好两行）
    let avail = ui.available_width();
    let gap = 12.0_f32;
    let per = 246.0_f32;
    let cols = (((avail + gap) / per).floor() as usize).clamp(1, 4);
    let w = (avail - gap * (cols as f32 - 1.0)) / cols as f32;

    // 第一行四张都不带副标题，保持等高；第二行统一带说明
    let cards: Vec<(&str, String, String, Tone)> = vec![
        ("总合同数", count.to_string(), String::new(), Tone::Neutral),
        (
            "履行中合同数",
            performing.to_string(),
            String::new(),
            Tone::Success,
        ),
        (
            "归档关闭合同数",
            archived.to_string(),
            String::new(),
            Tone::Neutral,
        ),
        (
            "合同金额合计",
            Money::from_cent(total).format_yuan(),
            String::new(),
            Tone::Neutral,
        ),
        (
            "有效计价基准",
            Money::from_cent(base).format_yuan(),
            "审计价优先".into(),
            Tone::Neutral,
        ),
        (
            "累计已付",
            Money::from_cent(paid).format_yuan(),
            "仅已实付".into(),
            Tone::Primary,
        ),
        (
            "综合支付率",
            rate.percent_string(),
            "已付 ÷ 基准".into(),
            Tone::Primary,
        ),
        (
            "超付风险",
            format!("{risk} 份"),
            "基准低于已付".into(),
            if risk > 0 {
                Tone::Danger
            } else {
                Tone::Success
            },
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
            ui.add_space(6.0);
        }
    });

    ui.add_space(4.0);

    // ---------------- 最近拨款（整宽、带表头）
    let mut open_pay: Option<String> = None;
    let mut open_contract: Option<String> = None;
    let mut print_pay: Option<String> = None;
    let mut print_material: Option<String> = None;
    // 批量打印：与拨款管理页共用 print_sel，遍历列表时先收集勾选变化
    let mut batch_print = false;
    let mut select_all = false;
    let mut clear_sel = false;
    let mut sel_toggle: Vec<(String, bool)> = Vec::new();

    let mut recent: Vec<RecentRow> = app
        .payments
        .iter()
        .map(|pm| {
            let s = app
                .summaries
                .iter()
                .find(|s| s.contract_id == pm.contract_id);
            let cname = s
                .map(|s| s.contract_name.clone())
                .unwrap_or_else(|| "—".into());
            let contractor = s
                .map(|s| s.contractor_name.clone())
                .unwrap_or_else(|| "—".into());
            (
                pm.id.clone(),
                pm.pay_date.clone(),
                pm.payment_no.clone(),
                pm.contract_id.clone(),
                cname,
                contractor,
                pm.amount_cent.cent(),
                pm.status == ctp_core::payment::PaymentStatus::Void,
            )
        })
        .collect();
    recent.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    // 只勾最近 6 条里未作废的单据；顺序与列表一致，批量打印按此顺序成页
    let payable_ids: Vec<String> = recent
        .iter()
        .take(6)
        .filter(|r| !r.7)
        .map(|r| r.0.clone())
        .collect();

    widgets::card(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        widgets::section_title(ui, &p, "最近拨款", "点单号看拨款详情，点合同名称看合同详情");
        ui.add_space(2.0);

        if recent.is_empty() {
            widgets::empty_state(ui, &p, "尚无拨款记录，去「拨款管理」登记第一笔。");
            return;
        }

        // 与「拨款管理 · 拨款明细」完全一致的文字链，共用同一套勾选集合
        let chosen = payable_ids
            .iter()
            .filter(|id| app.print_sel.contains(*id))
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
        ui.add_space(2.0);

        // 表头
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            head_cell(ui, &p, 24.0, "", false);
            head_cell(ui, &p, 86.0, "开单日期", false);
            head_cell(ui, &p, 104.0, "拨款单号", false);
            // R-58：合同名称 ×1.2（→ 368.16）、承包单位 ×1.2（→ 156），其余列宽不变
            head_cell(ui, &p, NAME_W, "合同名称", false);
            head_cell(ui, &p, CONTRACTOR_W, "承包单位", false);
            head_cell(ui, &p, 110.0, "金额", true);
            head_cell(ui, &p, 52.0, "状态", false);
            head_cell(ui, &p, 116.0, "操作", false);
        });
        ui.add_space(3.0);
        widgets::divider(ui, &p);

        for (i, (pay_id, date, no, contract_id, cname, contractor, amount, voided)) in
            recent.into_iter().take(6).enumerate()
        {
            let fill = if i % 2 == 1 {
                p.surface_alt
            } else {
                egui::Color32::TRANSPARENT
            };
            egui::Frame::none()
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(0.0, 2.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        cell(ui, 24.0, false, |ui| {
                            let paid = !voided;
                            let mut on = app.print_sel.contains(&pay_id);
                            let cb = ui.add_enabled(paid, egui::Checkbox::new(&mut on, ""));
                            if !paid {
                                cb.on_hover_text("作废单不可打印");
                            } else if cb.changed() {
                                sel_toggle.push((pay_id.clone(), on));
                            }
                        });
                        cell(ui, 86.0, false, |ui| {
                            ui.label(RichText::new(date).size(12.0).color(p.text_muted));
                        });
                        cell(ui, 104.0, false, |ui| {
                            if widgets::link(ui, &p, &no, 12.5).clicked() {
                                open_pay = Some(pay_id.clone());
                            }
                        });
                        cell(ui, NAME_W, false, |ui| {
                            // 合同名称即详情入口：悬停滚动查看全名，点击弹出该合同的详情弹窗
                            let r = widgets::marquee_sense(
                                ui,
                                &cname,
                                NAME_W,
                                12.5,
                                p.primary,
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("点击查看合同详情");
                            if r.clicked() {
                                open_contract = Some(contract_id.clone());
                            }
                        });
                        cell(ui, CONTRACTOR_W, false, |ui| {
                            widgets::marquee(ui, &contractor, CONTRACTOR_W, 12.0, p.text_muted);
                        });
                        cell(ui, 110.0, true, |ui| {
                            ui.label(
                                RichText::new(Money::from_cent(amount).format_yuan())
                                    .size(13.0)
                                    .strong()
                                    .color(p.text),
                            );
                        });
                        cell(ui, 52.0, false, |ui| {
                            let tone = if voided { Tone::Neutral } else { Tone::Success };
                            widgets::badge(ui, &p, if voided { "作废" } else { "已付" }, tone);
                        });
                        cell(ui, 116.0, false, |ui| {
                            // 与「拨款管理 · 拨款明细」操作列一致：文字链，作废单置灰不可点
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if voided {
                                ui.label(
                                    RichText::new("打印拨款单").size(11.0).color(p.text_muted),
                                )
                                .on_hover_text("已作废的拨款单不可打印");
                                ui.label(RichText::new("打印附件").size(11.0).color(p.text_muted))
                                    .on_hover_text("已作废的拨款单不可打印");
                            } else {
                                if widgets::link(ui, &p, "打印拨款单", 11.0)
                                    .on_hover_text("预览付款打印模板的 9 个合并域取值")
                                    .clicked()
                                {
                                    print_pay = Some(pay_id.clone());
                                }
                                if widgets::link(ui, &p, "打印附件", 11.0)
                                    .on_hover_text("中标通知书、审计报告等材料打印")
                                    .clicked()
                                {
                                    print_material = Some(contract_id.clone());
                                }
                            }
                        });
                    });
                });
        }
    });

    // ---------------- 交互落地
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
        app.print_sel.extend(payable_ids.iter().cloned());
    }
    if batch_print {
        let ids: Vec<String> = payable_ids
            .iter()
            .filter(|id| app.print_sel.contains(*id))
            .cloned()
            .collect();
        app.open_batch_print(ids);
    }
    if let Some(id) = open_pay {
        app.open_payment(&id);
    }
    if let Some(id) = open_contract {
        app.open_contract_detail(&id);
    }
    if let Some(id) = print_pay {
        if let Some(pm) = app.payments.iter().find(|x| x.id == id).cloned() {
            app.open_print_preview(&pm);
        }
    }
    if let Some(cid) = print_material {
        app.open_material_print(&cid);
    }
    if new_contract && app.require_write() {
        app.dialog = crate::dialogs::Dialog::NewContract;
    }
    if new_payment && app.require_write() {
        app.start_payment();
    }
}

fn cell(ui: &mut egui::Ui, width: f32, right: bool, add: impl FnOnce(&mut egui::Ui)) {
    let layout = if right {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    ui.allocate_ui_with_layout(egui::vec2(width, 24.0), layout, |ui| {
        ui.set_width(width);
        // 列间留 5px，避免金额与状态等相邻列贴在一起
        ui.add_space(5.0);
        add(ui);
    });
}

fn head_cell(ui: &mut egui::Ui, p: &crate::theme::Palette, width: f32, text: &str, right: bool) {
    cell(ui, width, right, |ui| {
        ui.label(RichText::new(text).size(11.5).color(p.text_muted));
    });
}
