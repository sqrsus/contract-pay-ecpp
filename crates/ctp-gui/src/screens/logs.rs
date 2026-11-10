//! 操作日志页：所有写库操作的留痕、筛选与清理。仅管理员可见（导航已挡）。
//!
//! 数据口径（R-31）：
//! * 列表 / 计数走 `ctp_app::logs`，与库里的 `sys_audit_log` 同源；
//! * 时间显示的是**本地时间**（存储为 UTC，查询层换算）；
//! * 清理必须二次确认，清完留一条 `CLEAR` 记录写明范围与条数。

use crate::screens::page_header;
use crate::widgets;
use crate::App;
use ctp_app::logs;
use ctp_store::audit::LogFilter;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

/// 单页最多取多少条。日志表有 5 万条上限，超出部分用「共 N 条」提示。
const PAGE_LIMIT: i64 = 500;

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    page_header(
        ui,
        &p,
        "操作日志",
        "所有写库操作的留痕：合同、拨款、附件、报表导出与设置；清理后也会留一条记录",
    );

    // ---------------- 查询：筛选条件变化时重查（与汇总统计同一模式）
    let key = (
        app.log_from.trim().to_string(),
        app.log_to.trim().to_string(),
        app.log_action.clone(),
        app.log_kw.trim().to_string(),
    );
    if app.log_key.as_ref() != Some(&key) {
        let f = view_filter(app);
        // R-39：4 个查询的错误不再被 unwrap_or_default 吞掉；
        // 失败时不保留旧数据当「正常结果」，给出可见警告并等顶栏「重试读取」。
        let result: Result<(Vec<ctp_store::audit::AuditLogRow>, i64, i64, Vec<String>), String> =
            app.read(|c| {
                Ok((
                    logs::list(c, &f, PAGE_LIMIT)?,
                    logs::count(c, &f)?,
                    logs::total(c)?,
                    logs::actions(c)?,
                ))
            });
        match result {
            Ok((rows, count, total, actions)) => {
                app.log_rows = rows;
                app.log_count = count;
                app.log_total = total;
                app.log_actions = actions;
            }
            Err(e) => app.warn_data("查询操作日志", e),
        }
        // 失败也记下这个 key，避免每帧重查把提示条刷屏；重试时由 retry_data 清空
        app.log_key = Some(key);
    }

    // ---------------- 筛选行
    let mut clear_range = false;
    let mut clear_all = false;
    ui.horizontal(|ui| {
        widgets::field_label(ui, &p, "日期");
        widgets::date_input(ui, "log_from", &p, &mut app.log_from, 104.0);
        ui.label(RichText::new("~").size(12.0).color(p.text_muted));
        widgets::date_input(ui, "log_to", &p, &mut app.log_to, 104.0);
        if widgets::link(ui, &p, "全部时间", 11.5).clicked() {
            app.log_from.clear();
            app.log_to.clear();
        }
        ui.add_space(12.0);
        widgets::field_label(ui, &p, "类型");
        let sel = app.log_action.clone();
        let sel_label = sel
            .as_ref()
            .map(|a| logs::action_label(a))
            .unwrap_or_else(|| "全部".to_string());
        egui::ComboBox::from_id_salt("log_action")
            .selected_text(sel_label)
            .width(150.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.log_action, None, "全部");
                for a in app.log_actions.clone() {
                    let label = logs::action_label(&a);
                    ui.selectable_value(&mut app.log_action, Some(a), label);
                }
            });
        ui.add_space(12.0);
        widgets::field_label(ui, &p, "关键词");
        ui.add_sized(
            [170.0, 28.0],
            egui::TextEdit::singleline(&mut app.log_kw)
                .hint_text("对象 / 内容 / 编号")
                .margin(egui::Margin::symmetric(8.0, 4.0)),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let range_ok = !app.log_from.trim().is_empty() || !app.log_to.trim().is_empty();
            if widgets::danger_solid_button(ui, &p, "全部清理", true).clicked() {
                clear_all = true;
            }
            if widgets::ghost_button(ui, &p, "按时间段清理")
                .on_hover_text("删除起止日期闭区间内的全部日志（不看类型与关键词）")
                .clicked()
                && range_ok
            {
                clear_range = true;
            }
        });
    });

    ui.add_space(6.0);
    ui.label(
        RichText::new(format!(
            "筛选命中 {} 条{} / 全库 {} 条（超过 {} 条自动裁掉最旧）",
            app.log_count,
            if app.log_count > PAGE_LIMIT {
                format!("，仅显示最新 {PAGE_LIMIT} 条")
            } else {
                String::new()
            },
            app.log_total,
            logs::AUDIT_KEEP_MAX
        ))
        .size(11.5)
        .color(p.text_muted),
    );
    ui.add_space(10.0);

    // ---------------- 列表
    if app.log_rows.is_empty() {
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            widgets::empty_state(ui, &p, "没有符合条件的日志");
        });
    } else {
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.x = 16.0;
            TableBuilder::new(ui)
                .striped(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::auto().at_least(150.0)) // 时间
                .column(Column::auto().at_least(96.0)) // 类型
                .column(Column::auto().at_least(72.0)) // 对象类型
                .column(Column::remainder().at_least(180.0)) // 操作对象
                .column(Column::remainder().at_least(220.0)) // 内容
                .column(Column::auto().at_least(70.0)) // 操作人
                .header(26.0, |mut h| {
                    for t in [
                        "时间",
                        "操作类型",
                        "对象类型",
                        "操作对象",
                        "操作内容",
                        "操作人",
                    ] {
                        h.col(|ui| widgets::th(ui, &p, t, false));
                    }
                })
                .body(|mut body| {
                    for r in &app.log_rows {
                        body.row(28.0, |mut row| {
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(&r.created_local)
                                        .size(12.0)
                                        .color(p.text_muted),
                                );
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(logs::action_label(&r.action))
                                        .size(12.0)
                                        .color(p.text),
                                );
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(logs::target_label(&r.target_type))
                                        .size(11.5)
                                        .color(p.text_muted),
                                );
                            });
                            row.col(|ui| {
                                let name = r
                                    .target_label
                                    .clone()
                                    .unwrap_or_else(|| r.target_id.clone());
                                widgets::cell_marquee(ui, &name, 12.0, p.text, egui::Align::LEFT);
                            });
                            row.col(|ui| {
                                let d = r.detail.clone().unwrap_or_default();
                                let (show, hover) = if d.is_empty() {
                                    ("—".to_string(), String::new())
                                } else {
                                    (d.clone(), d)
                                };
                                let resp = widgets::cell_marquee(
                                    ui,
                                    &show,
                                    12.0,
                                    p.text_muted,
                                    egui::Align::LEFT,
                                );
                                if !hover.is_empty() {
                                    resp.on_hover_text(hover);
                                }
                            });
                            row.col(|ui| {
                                widgets::cell_marquee(
                                    ui,
                                    &r.operator_id.clone().unwrap_or_default(),
                                    12.0,
                                    p.text_muted,
                                    egui::Align::LEFT,
                                );
                            });
                        });
                    }
                });
        });
    }

    // ---------------- 清理确认弹窗（二次确认；清完由服务层留 CLEAR 记录）
    if let Some((filter, desc)) = app.log_clear.clone() {
        let mut cancel = false;
        let mut submit = false;
        let n = app.read(|c| logs::count(c, &filter)).unwrap_or(0);
        // R-68（C14）：「全部清理」双保险——须输入确认词「清空」；且服务层
        // 会保底保留最近 200 条，这里提前告知用户。
        let is_clear_all = filter.is_empty();
        let word_ok = !is_clear_all || app.log_clear_word.trim() == "清空";
        egui::Window::new("确认清理日志")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .frame(
                egui::Frame::none()
                    .fill(p.surface)
                    .stroke(egui::Stroke::new(1.0_f32, p.border))
                    .rounding(egui::Rounding::same(14.0))
                    .inner_margin(egui::Margin::same(20.0)),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(430.0);
                ui.label(
                    RichText::new(desc.clone())
                        .size(14.0)
                        .strong()
                        .color(p.text),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "将删除 {n} 条日志，删除后不可恢复。{}",
                        if is_clear_all {
                            "系统会保底保留最近 200 条及本条清理留痕。"
                        } else {
                            ""
                        }
                    ))
                    .size(12.5)
                    .color(if n > 0 { p.danger } else { p.text_muted }),
                );
                if is_clear_all {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!(
                            "「全部清理」影响面大，请输入「清空」二字确认（已输入：{}）",
                            if app.log_clear_word.trim().is_empty() {
                                "—"
                            } else {
                                app.log_clear_word.trim()
                            }
                        ))
                        .size(11.5)
                        .color(p.text_muted),
                    );
                    ui.add_sized(
                        [ui.available_width(), 26.0],
                        egui::TextEdit::singleline(&mut app.log_clear_word)
                            .hint_text("输入：清空")
                            .margin(egui::Margin::symmetric(8.0, 4.0)),
                    );
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("清理完成后会自动留一条「清理日志」记录，写明范围与条数。")
                        .size(11.5)
                        .color(p.text_muted),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if widgets::danger_solid_button(ui, &p, "确认清理", n > 0 && word_ok).clicked()
                        && n > 0
                        && word_ok
                    {
                        submit = true;
                    }
                    if widgets::ghost_button(ui, &p, "取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            app.log_clear = None;
            app.log_clear_word.clear();
        }
        if submit && n > 0 {
            app.log_clear = None;
            app.log_clear_word.clear();
            let op = app.operator();
            match app.exec(move |c| logs::clear(c, &filter, op)) {
                Ok(n) => {
                    app.log_key = None;
                    app.success(format!("已清理 {n} 条日志（已留痕）"));
                }
                Err(e) => app.error(format!("清理失败：{e}")),
            }
        }
    }

    // ---------------- 清理入口动作
    if clear_range {
        let f = LogFilter {
            from: app.log_from.trim().to_string(),
            to: app.log_to.trim().to_string(),
            action: None,
            keyword: String::new(),
        };
        let desc = logs::describe(&f);
        app.log_clear = Some((f, desc));
    }
    if clear_all {
        app.log_clear = Some((LogFilter::default(), "全部清理".to_string()));
    }
}

/// 看列表用的筛选（类型 + 关键词 + 日期）；清理用的时间段筛选单独构造。
fn view_filter(app: &App) -> LogFilter {
    LogFilter {
        from: app.log_from.trim().to_string(),
        to: app.log_to.trim().to_string(),
        action: app.log_action.clone(),
        keyword: app.log_kw.trim().to_string(),
    }
}
