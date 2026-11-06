//! 汇总统计页（原「分类汇总」与「期间统计」合并）。
//!
//! 三个维度共用一套数据口径：只统计**已实付且未作废**的拨款单，日期落在
//! 用户选的闭区间内。屏幕表格与 Excel 导出读同一份 `ReportResult`（文档 4.4.2），
//! 不各自查库、不各自算合计。

use crate::theme::Palette;
use crate::widgets;
use crate::App;
use ctp_app::report::{Cell, ReportResult};
use ctp_core::date::is_iso_date;
use ctp_core::ledger::SummaryDim;
use ctp_core::money::Money;
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
    // 列间距 20px：中文表头在两列相邻时容易糊成一片
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
    tb    .header(26.0, |mut h| {
        for c in &result.columns {
            h.col(|ui| widgets::th(ui, p, c.title, c.right));
        }
    })
    .body(|mut body| {
        for row in &result.rows {
            body.row(29.0, |mut r| {
                for (ci, cell) in row.iter().enumerate() {
                    let right = result.columns[ci].right;
                    r.col(|ui| draw_cell(ui, p, cell, right, bar_width, false));
                }
            });
        }
        body.row(30.0, |mut r| {
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

/// 柱状图：横轴为分组名、纵轴为金额。数据点过多时不画（改成只看表）。
///
/// `h` 由调用方按剩余高度给定——页面没有整页滚动，宁可把图压矮，
/// 也要先保证表格连「合计」行完整可见。
fn bar_chart(ui: &mut egui::Ui, p: &Palette, points: &[(String, i64)], title: &str, h: f32) {
    widgets::card(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        widgets::section_title(ui, p, title, "单位：元");
        ui.add_space(10.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let n = points.len().max(1);
        let maxv = points.iter().map(|(_, v)| *v).max().unwrap_or(1).max(1);
        // 柱距封顶 130px，内容整体居中，避免分组少时两根柱子隔着一整屏
        let content_w = (n as f32 * 130.0).min(rect.width());
        let left = rect.left() + (rect.width() - content_w) * 0.5;
        let slot = content_w / n as f32;
        let bw = (slot * 0.46).clamp(8.0, 58.0);
        let base_y = rect.bottom() - 22.0;
        let top_pad = 20.0;
        painter.hline(
            rect.left()..=rect.right(),
            base_y,
            egui::Stroke::new(1.0_f32, p.border),
        );
        for (i, (label, cent)) in points.iter().enumerate() {
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
                label,
                egui::FontId::proportional(11.0),
                p.text_muted,
            );
            painter.text(
                egui::pos2(cx, base_y - bh.max(2.0) - 9.0),
                egui::Align2::CENTER_CENTER,
                Money::from_cent(*cent).format_yuan(),
                egui::FontId::proportional(10.5),
                p.text_muted,
            );
        }
    });
}

pub fn summary(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    let mut export = false;

    // ---------------- 页头：维度切换 + 区间 + 导出
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("汇总统计").size(19.0).strong().color(p.text));
            ui.label(
                RichText::new("只统计已实付单据，作废单不计；四个维度的合计口径完全一致")
                    .size(12.0)
                    .color(p.text_muted),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::ghost_button(ui, &p, "导出 Excel").clicked() {
                export = true;
            }
        });
    });
    ui.add_space(10.0);

    // 维度 Tab + 日期区间
    ui.horizontal(|ui| {
        for d in SummaryDim::all() {
            let on = d == app.summary_dim;
            let resp = if on {
                widgets::primary_button_sm(ui, &p, d.label())
            } else {
                widgets::ghost_button_sm(ui, &p, d.label())
            };
            if resp.clicked() && !on {
                app.summary_dim = d;
            }
        }

        ui.add_space(16.0);
        ui.label(RichText::new("统计区间").size(12.0).color(p.text_muted));
        ui.add_sized(
            [104.0, 28.0],
            egui::TextEdit::singleline(&mut app.summary_from)
                .hint_text("不限")
                .margin(egui::Margin::symmetric(7.0, 4.0)),
        );
        ui.label(RichText::new("至").size(12.0).color(p.text_muted));
        ui.add_sized(
            [104.0, 28.0],
            egui::TextEdit::singleline(&mut app.summary_to)
                .hint_text("不限")
                .margin(egui::Margin::symmetric(7.0, 4.0)),
        );

        // 三个常用区间，省得手打日期；「全部」清空两端
        let today = ctp_core::date::today_iso();
        let year = today.get(0..4).unwrap_or("2026").to_string();
        for (label, from, to) in [
            ("本月", format!("{}-01", &today[0..7]), today.clone()),
            ("本年", format!("{year}-01-01"), today.clone()),
            ("全部时间", String::new(), String::new()),
        ] {
            if widgets::link(ui, &p, label, 11.5).clicked() {
                app.summary_from = from;
                app.summary_to = to;
            }
        }
    });

    // 筛选行：项目状态 + 合同类型 + 关键词（R-32 口径 9b）
    ui.horizontal(|ui| {
        ui.label(RichText::new("项目状态").size(12.0).color(p.text_muted));
        let st_text = match app.summary_archived {
            Some(false) => "履行中",
            Some(true) => "归档关闭",
            None => "全部",
        };
        egui::ComboBox::from_id_salt("sum_status")
            .selected_text(st_text)
            .width(110.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.summary_archived, None, "全部");
                ui.selectable_value(&mut app.summary_archived, Some(false), "履行中");
                ui.selectable_value(&mut app.summary_archived, Some(true), "归档关闭");
            });
        ui.add_space(12.0);
        ui.label(RichText::new("合同类型").size(12.0).color(p.text_muted));
        let ty_text = app
            .summary_type
            .as_deref()
            .and_then(ctp_core::contract::ContractType::from_code)
            .map(|t| t.label())
            .unwrap_or("全部");
        egui::ComboBox::from_id_salt("sum_type")
            .selected_text(ty_text)
            .width(110.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.summary_type, None, "全部");
                for t in [
                    ctp_core::contract::ContractType::Construct,
                    ctp_core::contract::ContractType::Supervise,
                    ctp_core::contract::ContractType::Design,
                    ctp_core::contract::ContractType::Survey,
                    ctp_core::contract::ContractType::Supply,
                    ctp_core::contract::ContractType::Service,
                ] {
                    ui.selectable_value(
                        &mut app.summary_type,
                        Some(t.code().to_string()),
                        t.label(),
                    );
                }
            });
        ui.add_space(12.0);
        ui.label(RichText::new("关键词").size(12.0).color(p.text_muted));
        ui.add_sized(
            [170.0, 28.0],
            egui::TextEdit::singleline(&mut app.summary_kw)
                .hint_text("项目 / 承包单位 / 合同")
                .margin(egui::Margin::symmetric(8.0, 4.0)),
        );
    });
    ui.add_space(12.0);

    // ---------------- 取数（筛选或维度变了才重查）
    let key = (
        app.summary_dim,
        app.summary_from.trim().to_string(),
        app.summary_to.trim().to_string(),
        app.summary_archived,
        app.summary_type.clone(),
        app.summary_kw.trim().to_string(),
    );
    // 每端要么留空（不限）要么是合法日期；两端都给时起始不得晚于结束
    let side_ok = |s: &str| s.is_empty() || is_iso_date(s);
    let range_ok = side_ok(&key.1)
        && side_ok(&key.2)
        && (key.1.is_empty() || key.2.is_empty() || key.1 <= key.2);
    if app.summary_key.as_ref() != Some(&key) {
        if range_ok {
            let (dim, from, to, archived, ty, kw) = (
                key.0,
                key.1.clone(),
                key.2.clone(),
                key.3,
                key.4.clone(),
                key.5.clone(),
            );
            let filter = ctp_core::ledger::SummaryFilter {
                from,
                to,
                archived,
                contract_type: ty,
                keyword: kw,
            };
            app.summary = app
                .read(|c| ctp_app::service::summary_of(c, dim, &filter))
                .ok();
        } else {
            app.summary = None;
        }
        app.summary_key = Some(key);
    }

    if !range_ok {
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("统计区间日期格式不对：应为 YYYY-MM-DD，且起始不晚于结束（留空 = 不限）")
                    .size(13.0)
                    .color(p.danger),
            );
        });
    } else {
        match app.summary.clone() {
            None => {
                widgets::card(&p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    widgets::empty_state(ui, &p, "读取汇总数据失败，请检查数据库");
                });
            }
            Some(result) if result.is_empty() => {
                widgets::card(&p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    widgets::empty_state(
                        ui,
                        &p,
                        "当前区间与筛选条件下没有已实付的拨款单，放宽条件或先去「拨款管理」登记",
                    );
                });
            }
            Some(result) => {
                // 数据点不多时给一张柱状图；>31 个分组只保留表格，避免柱子糊成一排。
                // 图高按剩余高度让位：表头 26 + 数据行 29/行 + 合计 30 + 卡片与副标题约 90，
                // 剩余不足时先压图，低于 96 就干脆不画，保证「合计」行始终完整可见。
                let rows_need = 26.0 + result.rows.len() as f32 * 29.0 + 30.0 + 90.0;
                let chart_h = (ui.available_height() - rows_need - 24.0).clamp(0.0, 168.0);
                // 「按合同」维度多一列承包单位：金额在 col 3，其余维度在 col 2
                let amt_idx = if app.summary_dim == SummaryDim::Contract {
                    3
                } else {
                    2
                };
                if result.rows.len() <= 31 && chart_h >= 96.0 {
                    let points: Vec<(String, i64)> = result
                        .rows
                        .iter()
                        .map(|r| {
                            let label = r.first().map(Cell::display).unwrap_or_default();
                            // 合同名太长，图上只留前 9 个字
                            let label = if label.chars().count() > 10 {
                                let cut: String = label.chars().take(9).collect();
                                format!("{cut}…")
                            } else {
                                label
                            };
                            let cent = match r.get(amt_idx) {
                                Some(Cell::Money(v)) => v.cent(),
                                _ => 0,
                            };
                            (label, cent)
                        })
                        .collect();
                    let title = match app.summary_dim {
                        SummaryDim::Project => "各项目支付金额",
                        SummaryDim::Contractor => "各承包单位支付金额",
                        SummaryDim::Contract => "各合同支付金额",
                        SummaryDim::ContractType => "各合同类型支付金额",
                    };
                    bar_chart(ui, &p, &points, title, chart_h);
                    ui.add_space(8.0);
                }

                widgets::card(&p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(&result.subtitle)
                            .size(11.5)
                            .color(p.text_muted),
                    );
                    ui.add_space(8.0);
                    result_table(ui, &p, &result, 110.0);
                });
            }
        }
    }

    if export {
        app.export_summary();
    }
}
