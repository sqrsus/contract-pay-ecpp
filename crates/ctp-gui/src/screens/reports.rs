//! 报表页：会计分类汇总（V3）与支付期间统计（V4）。
//!
//! 屏幕表格、Excel 导出共用同一份 `ReportResult`（文档 4.4.2）：
//! 这里只负责把 `Cell` 画出来，不再各自查库或各自算合计。

use crate::screens::page_header_action;
use crate::theme::Palette;
use crate::widgets;
use crate::App;
use ctp_app::report::{Cell, ReportKind, ReportResult};
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

/// 按 `Cell` 的语义画一个单元格。数字列右对齐，比率列画进度条 + 百分比。
fn draw_cell(
    ui: &mut egui::Ui,
    p: &Palette,
    cell: &Cell,
    right: bool,
    bar_width: f32,
    strong: bool,
) {
    let mut text = RichText::new(cell.display()).size(12.5).color(p.text);
    if strong {
        text = text.strong();
    }
    match cell {
        Cell::Rate(s, bp) => {
            let frac = (*bp as f32 / 10_000.0).clamp(0.0, 1.0);
            widgets::bar(ui, p, frac, p.primary, bar_width, 7.0);
            let mut label = RichText::new(s)
                .size(11.5)
                .color(if strong { p.text } else { p.text_muted });
            if strong {
                label = label.strong();
            }
            ui.label(label);
        }
        _ if right => {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(text);
            });
        }
        _ => {
            ui.label(text);
        }
    }
}

/// 通用的结果表格：列配置、数据行、合计行全部来自 `ReportResult`。
fn result_table(ui: &mut egui::Ui, p: &Palette, result: &ReportResult, bar_width: f32) {
    let span = ui.available_rect_before_wrap();
    let mut total_y: Option<f32> = None;
    // 列间距 16px：中文表头在两列相邻时容易糊成一片
    ui.spacing_mut().item_spacing.x = 20.0;

    let mut tb = TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
    for c in &result.columns {
        tb = tb.column(if c.screen_remainder {
            Column::remainder().at_least(c.screen_width)
        } else {
            Column::auto().at_least(c.screen_width)
        });
    }
    tb.header(28.0, |mut h| {
        for c in &result.columns {
            h.col(|ui| widgets::th(ui, p, c.title, c.right));
        }
    })
    .body(|mut body| {
        for row in &result.rows {
            body.row(30.0, |mut r| {
                for (ci, cell) in row.iter().enumerate() {
                    let right = result.columns[ci].right;
                    r.col(|ui| draw_cell(ui, p, cell, right, bar_width, false));
                }
            });
        }
        body.row(32.0, |mut r| {
            for (ci, cell) in result.totals.iter().enumerate() {
                let right = result.columns[ci].right;
                r.col(|ui| {
                    if ci == 0 {
                        total_y = Some(ui.max_rect().top());
                    }
                    draw_cell(ui, p, cell, right, bar_width, true);
                });
            }
        });
    });

    // 合计行与明细之间的分隔线（表格绘制完成后按屏幕坐标补画）
    if let Some(y) = total_y {
        widgets::total_rule(ui, p, span.left(), span.right(), y - 1.0);
    }
}

pub fn category(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    let result = ReportResult::category(&app.categories);
    let export = page_header_action(ui, &p, result.title, result.subtitle, "导出 Excel");

    widgets::card(&p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if result.is_empty() {
            widgets::empty_state(ui, &p, "暂无分类数据");
            return;
        }
        result_table(ui, &p, &result, 70.0);
    });

    if export {
        app.export_report(ReportKind::CategorySummary);
    }
}

pub fn period(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    let result = ReportResult::period(&app.periods);
    let export = page_header_action(ui, &p, result.title, result.subtitle, "导出 Excel");

    if result.is_empty() {
        widgets::card(&p).show(ui, |ui| {
            widgets::empty_state(ui, &p, "暂无拨款数据");
        });
    } else {
        // 柱状图：数据同样取自 ReportResult，不与表格各算一套
        let points: Vec<(String, i64)> = result
            .rows
            .iter()
            .map(|r| {
                let month = r.first().map(Cell::display).unwrap_or_default();
                let cent = match r.get(2) {
                    Some(Cell::Money(v)) => v.cent(),
                    _ => 0,
                };
                (month, cent)
            })
            .collect();

        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::section_title(ui, &p, "月度支付金额", "单位：元");
            ui.add_space(10.0);
            let h = 196.0_f32;
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            let n = points.len().max(1);
            let maxv = points.iter().map(|(_, v)| *v).max().unwrap_or(1).max(1);
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
            for (i, (month, cent)) in points.iter().enumerate() {
                let cx = left + slot * (i as f32 + 0.5);
                let bh = (*cent as f32 / maxv as f32) * (base_y - rect.top() - top_pad);
                let bar = egui::Rect::from_min_max(
                    egui::pos2(cx - bw / 2.0, base_y - bh.max(2.0)),
                    egui::pos2(cx + bw / 2.0, base_y),
                );
                painter.rect_filled(bar, egui::Rounding::same(4.0), p.primary);
                painter.text(
                    egui::pos2(cx, base_y + 12.0),
                    egui::Align2::CENTER_CENTER,
                    month,
                    egui::FontId::proportional(11.0),
                    p.text_muted,
                );
                painter.text(
                    egui::pos2(cx, base_y - bh.max(2.0) - 9.0),
                    egui::Align2::CENTER_CENTER,
                    ctp_core::money::Money::from_cent(*cent).format_yuan(),
                    egui::FontId::proportional(10.5),
                    p.text_muted,
                );
            }
        });

        ui.add_space(12.0);

        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            result_table(ui, &p, &result, 110.0);
        });
    }

    if export {
        app.export_report(ReportKind::PeriodSummary);
    }
}
