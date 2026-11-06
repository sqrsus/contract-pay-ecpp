//! 登录页：浏览登录免密，管理登录校验密码。
//!
//! 登录状态只存在内存里（`App::role`），关掉程序即失效，下次启动重新登录。
//! 页面上不显示任何业务数据，避免未登录时就能看到台账内容。

use crate::widgets::{self, Tone};
use crate::App;
use ctp_app::auth::{self, Role};
use eframe::egui::{self, RichText};

pub fn draw(app: &mut App, ctx: &egui::Context) {
    let p = app.pal;
    // 是否仍是初始密码：只影响底部那行提示，登录动作本身不依赖它
    let still_initial = app
        .db
        .as_ref()
        .and_then(|c| auth::is_initial(c).ok())
        .unwrap_or(true);
    let db_ready = app.db.is_some();

    let mut browse = false;
    let mut admin = false;
    let mut submit = false;

    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(p.bg))
        .show(ctx, |ui| {
            let avail = ui.available_size();
            // 卡片高度随是否展开密码框变化，用它把卡片摆到垂直居中
            let card_h = if app.login_admin { 352.0 } else { 208.0 };
            ui.add_space(((avail.y - card_h) * 0.5).max(16.0));

            ui.vertical_centered(|ui| {
                widgets::card(&p).show(ui, |ui| {
                    ui.set_width(432.0);
                    ui.vertical_centered(|ui| {
                        // 品牌方块
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(46.0, 46.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, egui::Rounding::same(12.0), p.primary);
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "合",
                            egui::FontId::proportional(22.0),
                            p.on_primary,
                        );
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new("工程合同台账与拨款管理平台")
                                .size(18.0)
                                .strong()
                                .color(p.text),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("浏览可查看全部信息，管理可登记与修改数据")
                                .size(12.0)
                                .color(p.text_muted),
                        );
                    });

                    ui.add_space(16.0);
                    widgets::divider(ui, &p);
                    ui.add_space(14.0);

                    // 两个入口并排；未展开密码框时等高
                    ui.horizontal(|ui| {
                        let gap = 12.0;
                        let bw = (ui.available_width() - gap) * 0.5;
                        ui.spacing_mut().item_spacing.x = gap;
                        if widgets::ghost_button_sized(ui, &p, "浏览登录", bw, 38.0).clicked() {
                            browse = true;
                        }
                        if widgets::primary_button_sized(ui, &p, "管理登录", bw, 38.0, db_ready)
                            .clicked()
                        {
                            admin = true;
                        }
                    });

                    if app.login_admin {
                        ui.add_space(14.0);
                        widgets::field_label(ui, &p, "管理密码");
                        ui.add_space(2.0);
                        let resp = ui.add_sized(
                            [ui.available_width(), 34.0],
                            egui::TextEdit::singleline(&mut app.login_pw)
                                .password(true)
                                .hint_text("请输入管理密码")
                                .margin(egui::Margin::symmetric(9.0, 6.0)),
                        );
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            submit = true;
                        }
                        if app.login_err.is_some() {
                            ui.add_space(6.0);
                            let msg = app.login_err.clone().unwrap_or_default();
                            ui.label(RichText::new(msg).size(12.0).color(p.danger));
                        }
                        ui.add_space(10.0);
                        let can = !app.login_pw.trim().is_empty();
                        if widgets::primary_button_sized(
                            ui,
                            &p,
                            "进入管理",
                            ui.available_width(),
                            36.0,
                            can,
                        )
                        .clicked()
                            && can
                        {
                            submit = true;
                        }
                        ui.add_space(6.0);
                        if widgets::link(ui, &p, "返回选择", 11.5).clicked() {
                            app.login_admin = false;
                            app.login_pw.clear();
                            app.login_err = None;
                        }
                    }

                    if still_initial && app.login_admin {
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(
                                "首次使用：初始管理密码 0001，登录后请在「自检与设置」中修改。",
                            )
                            .size(11.0)
                            .color(p.text_muted),
                        );
                    }
                });
            });

            if let Some(err) = app.init_error.clone() {
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(format!("数据库不可用：{err}"))
                            .size(12.0)
                            .color(p.danger),
                    );
                });
            }
        });

    // ---------------- 动作落地
    if browse {
        app.role = Some(Role::Browse);
        app.login_err = None;
        app.login_pw.clear();
        app.login_admin = false;
        app.toast(Tone::Neutral, "已以浏览身份进入：仅可查看，修改操作会被拒绝");
    }
    if admin {
        app.login_admin = true;
        app.login_err = None;
    }
    if submit {
        let ok = app
            .db
            .as_ref()
            .and_then(|c| auth::verify(c, &app.login_pw).ok())
            .unwrap_or(false);
        if ok {
            app.role = Some(Role::Admin);
            app.login_pw.clear();
            app.login_err = None;
            app.login_admin = false;
            app.success("已以管理员身份进入");
        } else {
            app.login_err = Some("密码错误，请重新输入".into());
            app.login_pw.clear();
        }
    }
}
