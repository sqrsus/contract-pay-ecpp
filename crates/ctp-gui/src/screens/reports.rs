//! 报表页：会计分类汇总（V3）与支付期间统计（V4）。

use crate::screens::page_header;
use crate::widgets;
use crate::App;
use ctp_core::money::Money;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

pub fn category(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    page_header(
        ui,
        &p,
        "会计分类汇总",
        "按会计分类（ct_category）汇总，V3 口径；合同类别（工程款/材料款）不参与此处统计",
    );

    widgets::card(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if app.categories.is_empty() {
            widgets::empty_state(ui, &p, "暂无分类数据");
            return;
        }
        let tb: i64 = app.categories.iter().map(|c| c.base_amount.cent()).sum();
        let tp: i64 = app.categories.iter().map(|c| c.paid_amount.cent()).sum();
        let tu: i64 = app.categories.iter().map(|c| c.unpaid_amount.cent()).sum();
        let span = ui.available_rect_before_wrap();
        let mut total_y: Option<f32> = None;
        // 列间距 16px：中文表头在两列相邻时容易糊成一片
        ui.spacing_mut().item_spacing.x = 20.0;

        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().at_least(80.0))
            .column(Column::remainder().at_least(140.0))
            .column(Column::auto().at_least(70.0))
            .column(Column::auto().at_least(120.0))
            .column(Column::auto().at_least(120.0))
            .column(Column::auto().at_least(120.0))
            .column(Column::auto().at_least(120.0))
            .column(Column::auto().at_least(130.0))
            .header(28.0, |mut h| {
                h.col(|ui| widgets::th(ui, &p, "分类编码", false));
                h.col(|ui| widgets::th(ui, &p, "会计分类", false));
                for t in ["合同数", "有效基准", "审定金额", "已付", "未付"] {
                    h.col(|ui| widgets::th(ui, &p, t, true));
                }
                h.col(|ui| widgets::th(ui, &p, "支付进度", false));
            })
            .body(|mut body| {
                for c in &app.categories {
                    body.row(30.0, |mut row| {
                        row.col(|ui| {
                            ui.label(RichText::new(&c.category_code).size(12.0).color(p.text_muted));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&c.category_name).size(12.5).color(p.text));
                        });
                        row.col(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(c.contract_count.to_string())
                                            .size(12.0)
                                            .color(p.text),
                                    );
                                },
                            );
                        });
                        for v in [c.base_amount, c.audit_amount, c.paid_amount, c.unpaid_amount] {
                            row.col(|ui| {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(v.format_yuan())
                                                .size(12.5)
                                                .color(p.text),
                                        );
                                    },
                                );
                            });
                        }
                        row.col(|ui| {
                            widgets::bar(
                                ui,
                                &p,
                                c.pay_rate.bp() as f32 / 10_000.0,
                                p.primary,
                                70.0,
                                7.0,
                            );
                            ui.label(
                                RichText::new(c.pay_rate.percent_string())
                                    .size(11.5)
                                    .color(p.text_muted),
                            );
                        });
                    });
                }
                // 合计行
                body.row(32.0, |mut row| {
                    row.col(|ui| {
                        total_y = Some(ui.max_rect().top());
                        ui.label(RichText::new("").size(12.0));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new("合计").size(12.5).strong().color(p.text));
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(
                                    app.categories
                                        .iter()
                                        .map(|c| c.contract_count)
                                        .sum::<i64>()
                                        .to_string(),
                                )
                                .size(12.5)
                                .strong()
                                .color(p.text),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(Money::from_cent(tb).format_yuan())
                                    .size(12.5)
                                    .strong()
                                    .color(p.text),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(
                                    Money::from_cent(
                                        app.categories.iter().map(|c| c.audit_amount.cent()).sum(),
                                    )
                                    .format_yuan(),
                                )
                                .size(12.5)
                                .strong()
                                .color(p.text),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(Money::from_cent(tp).format_yuan())
                                    .size(12.5)
                                    .strong()
                                    .color(p.text),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(Money::from_cent(tu).format_yuan())
                                    .size(12.5)
                                    .strong()
                                    .color(p.text),
                            );
                        });
                    });
                    row.col(|ui| {
                        let frac = if tb > 0 { tp as f32 / tb as f32 } else { 0.0 };
                        widgets::bar(ui, &p, frac, p.primary, 70.0, 7.0);
                        ui.label(
                            RichText::new(format!("{:.2}%", frac * 100.0))
                                .size(11.5)
                                .strong()
                                .color(p.text),
                        );
                    });
                });
            });

        // 合计行与明细之间的分隔线（表格绘制完成后按屏幕坐标补画）
        if let Some(y) = total_y {
            widgets::total_rule(ui, &p, span.left(), span.right(), y - 1.0);
        }
    });
}

pub fn period(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    page_header(
        ui,
        &p,
        "支付期间统计",
        "按开单月份归集（V4 口径），仅统计已实付单据",
    );

    if app.periods.is_empty() {
        widgets::card(&p).show(ui, |ui| {
            widgets::empty_state(ui, &p, "暂无拨款数据");
        });
        return;
    }

    // 柱状图
    widgets::card(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        widgets::section_title(ui, &p, "月度支付金额", "单位：元");
        ui.add_space(10.0);
        let h = 196.0_f32;
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let n = app.periods.len().max(1);
        let maxv = app
            .periods
            .iter()
            .map(|r| r.amount.cent())
            .max()
            .unwrap_or(1)
            .max(1);
        // 柱距封顶 130px，内容整体居中，避免月份少时两根柱子隔着一整屏
        let content_w = (n as f32 * 130.0).min(rect.width());
        let left = rect.left() + (rect.width() - content_w) * 0.5;
        let slot = content_w / n as f32;
        let bw = (slot * 0.46).clamp(18.0, 58.0);
        let base_y = rect.bottom() - 22.0;
        let top_pad = 20.0;
        painter.hline(
            rect.left()..=rect.right(),
            base_y,
            egui::Stroke::new(1.0_f32, p.border),
        );
        for (i, r) in app.periods.iter().enumerate() {
            let cx = left + slot * (i as f32 + 0.5);
            let bh = (r.amount.cent() as f32 / maxv as f32) * (base_y - rect.top() - top_pad);
            let bar = egui::Rect::from_min_max(
                egui::pos2(cx - bw / 2.0, base_y - bh.max(2.0)),
                egui::pos2(cx + bw / 2.0, base_y),
            );
            painter.rect_filled(bar, egui::Rounding::same(4.0), p.primary);
            painter.text(
                egui::pos2(cx, base_y + 12.0),
                egui::Align2::CENTER_CENTER,
                &r.period_month,
                egui::FontId::proportional(11.0),
                p.text_muted,
            );
            painter.text(
                egui::pos2(cx, base_y - bh.max(2.0) - 9.0),
                egui::Align2::CENTER_CENTER,
                Money::from_cent(r.amount.cent()).format_yuan(),
                egui::FontId::proportional(10.5),
                p.text_muted,
            );
        }
    });

    ui.add_space(12.0);

    widgets::card(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let total: i64 = app.periods.iter().map(|r| r.amount.cent()).sum();
        let span = ui.available_rect_before_wrap();
        let mut total_y: Option<f32> = None;
        ui.spacing_mut().item_spacing.x = 20.0;
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(180.0).at_least(120.0))
            .column(Column::initial(120.0).at_least(96.0))
            .column(Column::initial(180.0).at_least(140.0))
            .column(Column::remainder().at_least(160.0))
            .header(28.0, |mut h| {
                h.col(|ui| widgets::th(ui, &p, "期间", false));
                h.col(|ui| widgets::th(ui, &p, "单据笔数", true));
                h.col(|ui| widgets::th(ui, &p, "支付金额", true));
                h.col(|ui| widgets::th(ui, &p, "占期间合计", false));
            })
            .body(|mut body| {
                for r in &app.periods {
                    body.row(30.0, |mut row| {
                        row.col(|ui| {
                            ui.label(RichText::new(&r.period_month).size(12.5).color(p.text));
                        });
                        row.col(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    RichText::new(r.payment_count.to_string())
                                        .size(12.0)
                                        .color(p.text),
                                );
                            });
                        });
                        row.col(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    RichText::new(r.amount.format_yuan())
                                        .size(12.5)
                                        .color(p.text),
                                );
                            });
                        });
                        row.col(|ui| {
                            let frac = if total > 0 {
                                r.amount.cent() as f32 / total as f32
                            } else {
                                0.0
                            };
                            widgets::bar(ui, &p, frac, p.primary, 110.0, 7.0);
                            ui.label(
                                RichText::new(format!("{:.2}%", frac * 100.0))
                                    .size(11.5)
                                    .color(p.text_muted),
                            );
                        });
                    });
                }
                body.row(32.0, |mut row| {
                    row.col(|ui| {
                        total_y = Some(ui.max_rect().top());
                        ui.label(RichText::new("合计").size(12.5).strong().color(p.text));
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(
                                    app.periods.iter().map(|r| r.payment_count).sum::<i64>().to_string(),
                                )
                                .size(12.5)
                                .strong()
                                .color(p.text),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(Money::from_cent(total).format_yuan())
                                    .size(12.5)
                                    .strong()
                                    .color(p.text),
                            );
                        });
                    });
                    row.col(|ui| {
                        // 与数据行同构：满格进度条 + 百分比
                        widgets::bar(ui, &p, 1.0, p.primary, 110.0, 7.0);
                        ui.label(
                            RichText::new(if total > 0 { "100.00%" } else { "0.00%" })
                                .size(11.5)
                                .strong()
                                .color(p.text),
                        );
                    });
                });
            });
        if let Some(y) = total_y {
            widgets::total_rule(ui, &p, span.left(), span.right(), y - 1.0);
        }
    });
}
