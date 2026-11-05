//! 各功能页的绘制入口。

pub mod checks;
pub mod contracts;
pub mod dashboard;
pub mod payments;
pub mod reports;

use crate::theme::Palette;
use crate::{App, Screen};
use eframe::egui;

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    match app.screen {
        Screen::Dashboard => dashboard::draw(app, ui),
        Screen::Contracts => contracts::draw(app, ui),
        Screen::Payments => payments::draw(app, ui),
        Screen::Reports => reports::category(app, ui),
        Screen::Periods => reports::period(app, ui),
        Screen::Checks => checks::draw(app, ui),
    }
}

/// 页面标题行。
pub(crate) fn page_header(ui: &mut egui::Ui, p: &Palette, title: &str, subtitle: &str) {
    ui.label(egui::RichText::new(title).size(19.0).strong().color(p.text));
    ui.label(egui::RichText::new(subtitle).size(12.0).color(p.text_muted));
    ui.add_space(10.0);
}

/// 标题行右侧带一个动作按钮（报表页的「导出 Excel」）。
///
/// 返回按钮是否被点击；调用方在绘制完表格、借用释放后再执行副作用。
pub(crate) fn page_header_action(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    subtitle: &str,
    action: &str,
) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(title).size(19.0).strong().color(p.text));
            ui.label(egui::RichText::new(subtitle).size(12.0).color(p.text_muted));
        });
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if crate::widgets::ghost_button_sm(ui, p, action).clicked() {
                    clicked = true;
                }
            },
        );
    });
    ui.add_space(10.0);
    clicked
}
