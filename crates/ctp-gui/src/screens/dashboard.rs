//! 总览页：关键指标、风险提示、最近拨款、分类占比。

use crate::screens::page_header;
use crate::widgets::{self, Tone};
use crate::App;
use ctp_core::money::Money;
use ctp_core::rate::Rate;
use eframe::egui::{self, RichText};

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    page_header(
        ui,
        &p,
        "总览",
        "已付 = 已实付单据净额（作废单不计）；有效基准 = 审计价 > 0 ? 审计价 : 合同金额",
    );

    if app.summaries.is_empty() {
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("还没有合同数据").size(16.0).strong().color(p.text));
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "可以直接载入一套演示数据（1 个项目、3 份合同、若干拨款单），也可以从「合同台账」新建。",
                )
                .size(12.5)
                .color(p.text_muted),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &p, "载入演示数据", true).clicked() {
                    match app.exec(|c| ctp_app::demo::seed_demo(c, "界面用户")) {
                        Ok(()) => {
                            app.refresh();
                            app.success("演示数据已载入");
                        }
                        Err(e) => app.error(format!("载入演示数据失败：{e}")),
                    }
                }
                if widgets::ghost_button(ui, &p, "新建合同").clicked() {
                    app.dialog = crate::dialogs::Dialog::NewContract;
                }
            });
        });
        return;
    }

    let count = app.summaries.len();
    let total: i64 = app.summaries.iter().map(|s| s.total_amount.cent()).sum();
    let base: i64 = app.summaries.iter().map(|s| s.base_amount.cent()).sum();
    let paid: i64 = app.summaries.iter().map(|s| s.paid_amount.cent()).sum();
    let audited = app.summaries.iter().filter(|s| s.is_audited).count();
    let risk = app.summaries.iter().filter(|s| s.is_inverted).count();
    let rate = Rate::ratio(Money::from_cent(paid), Money::from_cent(base.max(1)));

    // KPI 卡片：按可用宽度自适应列数
    let avail = ui.available_width();
    let gap = 12.0_f32;
    let per = 252.0_f32;
    let cols = (((avail + gap) / per).floor() as usize).clamp(1, 3);
    let w = (avail - gap * (cols as f32 - 1.0)) / cols as f32;

    let cards: Vec<(&str, String, String, Tone)> = vec![
        ("合同份数", count.to_string(), format!("已审定 {audited} 份"), Tone::Neutral),
        ("合同金额合计", Money::from_cent(total).format_yuan(), "暂定金额".into(), Tone::Neutral),
        ("有效计价基准", Money::from_cent(base).format_yuan(), "审计价优先".into(), Tone::Neutral),
        ("累计已付", Money::from_cent(paid).format_yuan(), "仅已实付".into(), Tone::Primary),
        ("综合支付率", rate.percent_string(), "已付 ÷ 基准".into(), Tone::Primary),
        (
            "超付风险",
            format!("{risk} 份"),
            "基准低于已付".into(),
            if risk > 0 { Tone::Danger } else { Tone::Success },
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

    ui.add_space(16.0);

    // 左：最近拨款；右：分类占比
    let wide = avail > 900.0;
    ui.horizontal_top(|ui| {
        let left_w = if wide { (avail * 0.58).max(360.0) } else { avail };
        ui.allocate_ui_with_layout(
            egui::vec2(left_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                widgets::card(&p).show(ui, |ui| {
                    ui.set_width(left_w - 34.0);
                    widgets::section_title(ui, &p, "最近拨款", "点单号看详情，点合同编号跳到台账");
                    ui.add_space(6.0);
                    // (单 ID, 开单日期, 单号, 合同 ID, 合同编号, 金额, 是否作废)
                    let mut recent: Vec<(String, String, String, String, String, i64, bool)> = app
                        .payments
                        .iter()
                        .map(|pm| {
                            let cno = app
                                .summaries
                                .iter()
                                .find(|s| s.contract_id == pm.contract_id)
                                .map(|s| s.contract_no.clone())
                                .unwrap_or_else(|| "—".into());
                            (
                                pm.id.clone(),
                                pm.pay_date.clone(),
                                pm.payment_no.clone(),
                                pm.contract_id.clone(),
                                cno,
                                pm.amount_cent.cent(),
                                pm.status == ctp_core::payment::PaymentStatus::Void,
                            )
                        })
                        .collect();
                    recent.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
                    if recent.is_empty() {
                        widgets::empty_state(ui, &p, "尚无拨款记录，去「拨款管理」登记第一笔。");
                    } else {
                        let mut open_pay: Option<String> = None;
                        let mut goto_contract: Option<String> = None;
                        for (i, (pay_id, date, no, contract_id, cno, amount, voided)) in
                            recent.into_iter().take(6).enumerate()
                        {
                            if i > 0 {
                                widgets::divider(ui, &p);
                            }
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(date).size(12.0).color(p.text_muted));
                                if widgets::link(ui, &p, &no, 12.5).clicked() {
                                    open_pay = Some(pay_id);
                                }
                                if cno != "—" && widgets::link_muted(ui, &p, &cno, 12.0).clicked() {
                                    goto_contract = Some(contract_id);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let tone = if voided {
                                            Tone::Neutral
                                        } else {
                                            Tone::Success
                                        };
                                        widgets::badge(
                                            ui,
                                            &p,
                                            if voided { "作废" } else { "已付" },
                                            tone,
                                        );
                                        ui.label(
                                            RichText::new(
                                                Money::from_cent(amount).format_yuan(),
                                            )
                                            .size(13.5)
                                            .strong()
                                            .color(p.text),
                                        );
                                    },
                                );
                            });
                        }
                        if let Some(id) = open_pay {
                            app.open_payment(&id);
                        }
                        if let Some(id) = goto_contract {
                            app.goto_contract(&id);
                        }
                    }
                });
            },
        );

        if wide {
            ui.add_space(12.0);
            let right_w = avail - left_w - 12.0;
            ui.allocate_ui_with_layout(
                egui::vec2(right_w, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    widgets::card(&p).show(ui, |ui| {
                        ui.set_width(right_w - 34.0);
                        widgets::section_title(ui, &p, "会计分类占比", "已付 ÷ 有效基准");
                        ui.add_space(8.0);
                        if app.categories.is_empty() {
                            widgets::empty_state(ui, &p, "暂无分类数据");
                        } else {
                            let max_base = app
                                .categories
                                .iter()
                                .map(|c| c.base_amount.cent())
                                .max()
                                .unwrap_or(1)
                                .max(1);
                            for c in &app.categories {
                                ui.label(
                                    RichText::new(&c.category_name)
                                        .size(12.5)
                                        .color(p.text),
                                );
                                ui.add_space(3.0);
                                let bw = (right_w - 40.0).max(60.0);
                                widgets::bar(
                                    ui,
                                    &p,
                                    c.base_amount.cent() as f32 / max_base as f32,
                                    p.primary,
                                    bw,
                                    8.0,
                                );
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "已付 {} · {}",
                                            c.paid_amount.format_yuan(),
                                            c.pay_rate.percent_string()
                                        ))
                                        .size(11.0)
                                        .color(p.text_muted),
                                    );
                                });
                                ui.add_space(8.0);
                            }
                        }
                    });
                },
            );
        }
    });
}
