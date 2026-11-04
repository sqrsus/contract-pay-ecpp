//! 自检页：六项口径检查 + 超付容差设置。

use crate::screens::page_header;
use crate::widgets::{self, Tone};
use crate::App;
use ctp_app::service;
use ctp_core::rate::Rate;
use eframe::egui::{self, RichText};

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    page_header(
        ui,
        &p,
        "自检与设置",
        "六项检查与命令行 ctp selfcheck 同源，用来确认视图口径、表结构与范围台账没有漂移",
    );

    let mut run = false;
    let mut save_tol = false;

    ui.horizontal(|ui| {
        if widgets::primary_button(ui, &p, "运行自检", true).clicked() {
            run = true;
        }
        if let Some(r) = &app.report {
            let tone = if r.all_passed() { Tone::Success } else { Tone::Danger };
            widgets::badge(
                ui,
                &p,
                &format!(
                    "{} / {} 通过",
                    r.total() - r.failed(),
                    r.total()
                ),
                tone,
            );
            ui.label(
                RichText::new(format!("结构版本 {}", r.schema_version))
                    .size(11.5)
                    .color(p.text_muted),
            );
        } else {
            ui.label(
                RichText::new("尚未运行").size(11.5).color(p.text_muted),
            );
        }
    });

    ui.add_space(12.0);

    let avail = ui.available_width();
    ui.horizontal_top(|ui| {
        let left_w = if avail > 820.0 { avail - 312.0 } else { avail };
        ui.allocate_ui_with_layout(
            egui::vec2(left_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| match &app.report {
                None => {
                    widgets::card(&p).show(ui, |ui| {
                        ui.set_width(left_w - 34.0);
                        widgets::empty_state(
                            ui,
                            &p,
                            "点击「运行自检」检查六项：支付比率、表结构、范围台账、外键、已付口径、视图行数",
                        );
                    });
                }
                Some(r) => {
                    for item in &r.items {
                        widgets::card(&p).show(ui, |ui| {
                            ui.set_width(left_w - 34.0);
                            ui.horizontal(|ui| {
                                let tone = if item.passed { Tone::Success } else { Tone::Danger };
                                widgets::badge(
                                    ui,
                                    &p,
                                    if item.passed { "通过" } else { "未通过" },
                                    tone,
                                );
                                ui.label(
                                    RichText::new(item.name).size(13.5).strong().color(p.text),
                                );
                            });
                            ui.add_space(4.0);
                            for line in item.detail.lines() {
                                ui.label(
                                    RichText::new(line).size(12.0).color(p.text_muted),
                                );
                            }
                        });
                        ui.add_space(8.0);
                    }
                }
            },
        );

        if avail > 820.0 {
            ui.add_space(12.0);
            ui.allocate_ui_with_layout(
                egui::vec2(300.0, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    widgets::card(&p).show(ui, |ui| {
                        ui.set_width(266.0);
                        widgets::section_title(ui, &p, "超付容差", "上限 = 基准 ×（1 + 容差）");
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!("当前 {} bp（{}%）", app.tolerance_bp, app.tolerance_bp as f64 / 100.0))
                                .size(12.5)
                                .color(p.text),
                        );
                        ui.add_space(8.0);
                        widgets::field_label(ui, &p, "新的容差（万分比，0–1000）");
                        ui.add_sized(
                            [180.0, 30.0],
                            egui::TextEdit::singleline(&mut app.tol_input)
                                .margin(egui::Margin::symmetric(8.0, 4.0)),
                        );
                        ui.add_space(8.0);
                        let bp = app.tol_input.trim().parse::<i32>().ok();
                        let ok = bp.map(|v| (0..=1000).contains(&v)).unwrap_or(false);
                        if widgets::primary_button(ui, &p, "保存容差", ok).clicked() && ok {
                            save_tol = true;
                        }
                        ui.add_space(8.0);
                        widgets::divider(ui, &p);
                        ui.label(
                            RichText::new(
                                "容差只影响拦截上限，不改变已记录流水；建议保持 0–100 bp。",
                            )
                            .size(11.0)
                            .color(p.text_muted),
                        );
                    });

                    ui.add_space(12.0);
                    widgets::card(&p).show(ui, |ui| {
                        ui.set_width(266.0);
                        widgets::section_title(ui, &p, "数据源", "");
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(&app.db_path)
                                .size(11.0)
                                .color(p.text_muted),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("与命令行 ctp 使用同一个数据库文件。")
                                .size(11.0)
                                .color(p.text_muted),
                        );
                    });
                },
            );
        }
    });

    if run {
        let res = app.db.as_ref().and_then(|c| ctp_app::checks::run(c).ok());
        match res {
            Some(r) => {
                let failed = r.failed();
                app.report = Some(r);
                if failed == 0 {
                    app.success("自检通过：六项检查全部合格");
                } else {
                    app.error(format!("自检发现 {failed} 项未通过"));
                }
            }
            None => app.error("自检失败：数据库不可用"),
        }
    }

    if save_tol {
        if let Ok(bp) = app.tol_input.trim().parse::<i32>() {
            let rate = Rate::from_bp(bp);
            match app.exec(move |c| service::set_tolerance(c, rate, "界面用户")) {
                Ok(()) => {
                    app.refresh();
                    app.success(format!("容差已设为 {bp} bp"));
                }
                Err(e) => app.error(format!("保存容差失败：{e}")),
            }
        }
    }
}
