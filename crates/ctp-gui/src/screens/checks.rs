//! 自检页：六项口径检查 + 管理密码。
//!
//! R-33 起超付容差固定为 0，不再提供设置入口；右栏只保留两张卡。

use crate::screens::page_header;
use crate::widgets::{self, Tone};
use crate::App;
use ctp_app::auth;
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
    let mut save_pw = false;

    ui.horizontal(|ui| {
        if widgets::primary_button(ui, &p, "运行自检", true).clicked() {
            run = true;
        }
        if let Some(r) = &app.report {
            let tone = if r.all_passed() {
                Tone::Success
            } else {
                Tone::Danger
            };
            widgets::badge(
                ui,
                &p,
                &format!("{} / {} 通过", r.total() - r.failed(), r.total()),
                tone,
            );
            ui.label(
                RichText::new(format!("结构版本 {}", r.schema_version))
                    .size(11.5)
                    .color(p.text_muted),
            );
        } else {
            ui.label(RichText::new("尚未运行").size(11.5).color(p.text_muted));
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
                        widgets::section_title(ui, &p, "管理密码", "修改后立即生效");
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("浏览用户没有此页；忘记密码无法找回，只能删库重建。")
                                .size(11.0)
                                .color(p.text_muted),
                        );
                        ui.add_space(8.0);
                        for (label, buf) in [
                            ("旧密码", &mut app.pw_old),
                            ("新密码（4–20 位）", &mut app.pw_new),
                            ("确认新密码", &mut app.pw_new2),
                        ] {
                            widgets::field_label(ui, &p, label);
                            ui.add_sized(
                                [180.0, 30.0],
                                egui::TextEdit::singleline(buf)
                                    .password(true)
                                    .margin(egui::Margin::symmetric(8.0, 4.0)),
                            );
                            ui.add_space(4.0);
                        }
                        let all_filled = !app.pw_old.is_empty()
                            && !app.pw_new.is_empty()
                            && !app.pw_new2.is_empty();
                        ui.add_space(4.0);
                        if widgets::primary_button(ui, &p, "修改密码", all_filled).clicked()
                            && all_filled
                        {
                            save_pw = true;
                        }
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
        // R-39：区分「数据库没打开」与「自检执行报错」，后者要带出真实错误
        let res = match app.db.as_ref() {
            None => Err("数据库未打开".to_string()),
            Some(c) => ctp_app::checks::run(c).map_err(|e| e.to_string()),
        };
        match res {
            Ok(r) => {
                let failed = r.failed();
                app.report = Some(r);
                if failed == 0 {
                    app.success("自检通过：六项检查全部合格");
                } else {
                    app.error(format!("自检发现 {failed} 项未通过"));
                }
            }
            Err(e) => app.warn_data("运行口径自检", e),
        }
    }

    if save_pw {
        let old = app.pw_old.clone();
        let new = app.pw_new.clone();
        let new2 = app.pw_new2.clone();
        // 先验旧密码：输错旧密码不该先被新密码的格式问题挡回来
        let old_ok = app.read(|c| auth::verify(c, &old)).unwrap_or(false);
        if !old_ok {
            app.error("旧密码不正确");
        } else if let Err(msg) = auth::validate_new_password(&new) {
            app.error(msg);
        } else if new != new2 {
            app.error("两次输入的新密码不一致");
        } else {
            match app.exec(|c| auth::change(c, &new, "管理员")) {
                Ok(()) => {
                    app.pw_old.clear();
                    app.pw_new.clear();
                    app.pw_new2.clear();
                    app.success("管理密码已修改，下次登录请用新密码");
                }
                Err(e) => app.error(format!("修改密码失败：{e}")),
            }
        }
    }
}
