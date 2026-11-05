//! 通用视觉组件：卡片、KPI、徽标、进度条、按钮。
//!
//! 所有组件只依赖 `Palette`，因此浅色/深色两套主题下表现一致。

use crate::theme::Palette;
use eframe::egui::{
    self, Color32, Margin, Response, RichText, Rounding, Stroke, Ui, Vec2,
};

/// 语义色调。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Primary,
    Success,
    Warn,
    Danger,
}

impl Tone {
    /// (前景色, 柔和底色)
    pub fn colors(self, p: &Palette) -> (Color32, Color32) {
        match self {
            Tone::Neutral => (p.text_muted, p.surface_alt),
            Tone::Primary => (p.primary, p.primary_soft),
            Tone::Success => (p.success, p.success_soft),
            Tone::Warn => (p.warn, p.warn_soft),
            Tone::Danger => (p.danger, p.danger_soft),
        }
    }
}

/// 标准卡片容器。
pub fn card(p: &Palette) -> egui::Frame {
    egui::Frame::none()
        .fill(p.surface)
        .stroke(Stroke::new(1.0_f32, p.border))
        .rounding(Rounding::same(12.0))
        .inner_margin(Margin::same(16.0))
}

/// 无边框、仅底色的容器（用于提示条、内嵌区块）。
pub fn soft_box(_p: &Palette, fill: Color32) -> egui::Frame {
    egui::Frame::none()
        .fill(fill)
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::same(12.0))
}

/// 区块标题 + 说明。
pub fn section_title(ui: &mut Ui, p: &Palette, title: &str, hint: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(16.0).strong().color(p.text));
        if !hint.is_empty() {
            ui.add_space(2.0);
            ui.label(RichText::new(hint).size(12.0).color(p.text_muted));
        }
    });
    ui.add_space(2.0);
}

/// KPI 卡：小标题 + 大数字 + 说明。
pub fn kpi(
    ui: &mut Ui,
    p: &Palette,
    width: f32,
    label: &str,
    value: &str,
    sub: &str,
    tone: Tone,
) {
    let (fg, _) = tone.colors(p);
    card(p).show(ui, |ui| {
        // 卡片内边距 16×2，这里扣掉，保证外框总宽等于调用方给的 width
        ui.set_width((width - 32.0).max(80.0));
        ui.label(RichText::new(label).size(12.0).color(p.text_muted));
        ui.add_space(6.0);
        let color = if tone == Tone::Neutral { p.text } else { fg };
        ui.label(RichText::new(value).size(22.0).strong().color(color));
        if !sub.is_empty() {
            ui.add_space(2.0);
            ui.label(RichText::new(sub).size(11.5).color(p.text_muted));
        }
    });
}

/// 胶囊徽标。
pub fn badge(ui: &mut Ui, p: &Palette, text: &str, tone: Tone) {
    let (fg, soft) = tone.colors(p);
    egui::Frame::none()
        .fill(soft)
        .rounding(Rounding::same(999.0))
        .inner_margin(Margin::symmetric(9.0, 2.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(fg));
        });
}

/// 细进度条。
pub fn bar(ui: &mut Ui, p: &Palette, frac: f32, color: Color32, width: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, Rounding::same(height / 2.0), p.surface_alt);
    let w = width * frac.clamp(0.0, 1.0);
    if w > 0.5 {
        let r = egui::Rect::from_min_size(rect.min, Vec2::new(w, height));
        painter.rect_filled(r, Rounding::same(height / 2.0), color);
    }
}

/// 主按钮：实心主色，悬停提亮、按下加深、禁用置灰。
pub fn primary_button(ui: &mut Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    if !enabled {
        return ui.add(
            egui::Button::new(RichText::new(text).size(14.0).color(p.text_muted))
                .fill(p.surface_alt)
                .stroke(Stroke::new(1.0_f32, p.border))
                .rounding(Rounding::same(8.0))
                .min_size(Vec2::new(0.0, 32.0)),
        );
    }
    ui.scope(|ui| {
        let w = &mut ui.style_mut().visuals.widgets;
        w.inactive.weak_bg_fill = p.primary;
        w.inactive.bg_fill = p.primary;
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.fg_stroke = Stroke::new(1.0_f32, p.on_primary);
        w.hovered.weak_bg_fill = p.primary_hover;
        w.hovered.bg_fill = p.primary_hover;
        w.hovered.bg_stroke = Stroke::NONE;
        w.hovered.fg_stroke = Stroke::new(1.0_f32, p.on_primary);
        w.active.weak_bg_fill = p.primary_active;
        w.active.bg_fill = p.primary_active;
        w.active.bg_stroke = Stroke::NONE;
        w.active.fg_stroke = Stroke::new(1.0_f32, p.on_primary);
        ui.add(
            egui::Button::new(RichText::new(text).size(14.0).color(p.on_primary))
                .rounding(Rounding::same(8.0))
                .min_size(Vec2::new(0.0, 32.0)),
        )
    })
    .inner
}

/// 次要按钮：白底描边，悬停变浅主色。
pub fn ghost_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(13.5).color(p.text))
            .fill(p.surface)
            .stroke(Stroke::new(1.0_f32, p.border))
            .rounding(Rounding::same(8.0))
            .min_size(Vec2::new(0.0, 30.0)),
    )
}

/// 危险主按钮：实心红底，悬停加深、按下更深、禁用置灰。
/// 用于「确认作废」这类不可逆操作，禁用时必须明显不像可点。
pub fn danger_solid_button(ui: &mut Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    if !enabled {
        return ui.add(
            egui::Button::new(RichText::new(text).size(14.0).color(p.text_muted))
                .fill(p.surface_alt)
                .stroke(Stroke::new(1.0_f32, p.border))
                .rounding(Rounding::same(8.0))
                .min_size(Vec2::new(0.0, 32.0)),
        );
    }
    ui.scope(|ui| {
        let w = &mut ui.style_mut().visuals.widgets;
        for (state, fill) in [
            (&mut w.inactive, p.danger),
            (&mut w.hovered, p.danger_hover),
            (&mut w.active, p.danger_active),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke = Stroke::new(1.0_f32, p.on_primary);
        }
        ui.add(
            egui::Button::new(RichText::new(text).size(14.0).color(p.on_primary))
                .rounding(Rounding::same(8.0))
                .min_size(Vec2::new(0.0, 32.0)),
        )
    })
    .inner
}

/// 紧凑次要按钮：用在详情卡这类宽度受限的行内。
pub fn ghost_button_sm(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ghost_button_sm_on(ui, p, text, true)
}

/// 紧凑次要按钮（可禁用）：禁用时置灰描边，明显不像可点。
pub fn ghost_button_sm_on(ui: &mut Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    if !enabled {
        return ui.add(
            egui::Button::new(RichText::new(text).size(12.5).color(p.text_muted))
                .fill(p.surface_alt)
                .stroke(Stroke::new(1.0_f32, p.border))
                .rounding(Rounding::same(7.0))
                .min_size(Vec2::new(0.0, 28.0)),
        );
    }
    ui.add(
        egui::Button::new(RichText::new(text).size(12.5).color(p.text))
            .fill(p.surface)
            .stroke(Stroke::new(1.0_f32, p.border))
            .rounding(Rounding::same(7.0))
            .min_size(Vec2::new(0.0, 28.0)),
    )
}

/// 紧凑主按钮：详情卡里的「设为生效」这类唯一主行动。
pub fn primary_button_sm(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.scope(|ui| {
        let w = &mut ui.style_mut().visuals.widgets;
        for (state, fill) in [
            (&mut w.inactive, p.primary),
            (&mut w.hovered, p.primary_hover),
            (&mut w.active, p.primary_active),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke = Stroke::new(1.0_f32, p.on_primary);
        }
        ui.add(
            egui::Button::new(RichText::new(text).size(12.5).color(p.on_primary))
                .rounding(Rounding::same(7.0))
                .min_size(Vec2::new(0.0, 28.0)),
        )
    })
    .inner
}

/// 紧凑危险按钮。
pub fn danger_button_sm(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(12.5).color(p.danger))
            .fill(p.surface)
            .stroke(Stroke::new(1.0_f32, p.danger))
            .rounding(Rounding::same(7.0))
            .min_size(Vec2::new(0.0, 28.0)),
    )
}

/// 表单字段标题。
pub fn field_label(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(p.text_muted));
}

/// 一行「标签 — 值」，用于详情面板。
pub fn kv(ui: &mut Ui, p: &Palette, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.5).color(p.text_muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).size(13.0).color(p.text));
        });
    });
}

/// 水平分隔线。
pub fn divider(ui: &mut Ui, p: &Palette) {
    ui.add_space(2.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, Rounding::same(0.5), p.border);
    ui.add_space(2.0);
}

/// 空状态提示。
///
/// 用 `with_layout` 而不是 `vertical_centered`：后者会按可用空间撑开，
/// 放在卡片里会把卡片拉成一整屏高。
pub fn empty_state(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add_space(26.0);
    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
        ui.label(RichText::new(text).size(13.5).color(p.text_muted));
    });
    ui.add_space(26.0);
}

/// 表格表头单元。数值列必须传 `right = true`，否则表头与右对齐的数值会错开。
pub fn th(ui: &mut Ui, p: &Palette, text: &str, right: bool) {
    let label = RichText::new(text).size(12.0).color(p.text_muted);
    if right {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(label);
        });
    } else {
        ui.label(label);
    }
}

/// 可点击文字：列表里的编号。悬停出下划线 + 手型光标。
pub fn link(ui: &mut Ui, p: &Palette, text: &str, size: f32) -> Response {
    link_colored(ui, text, size, p.primary, p.primary)
}

/// 弱化的可点击文字：常态灰、悬停主色下划线，避免和主编号抢注意力。
pub fn link_muted(ui: &mut Ui, p: &Palette, text: &str, size: f32) -> Response {
    link_colored(ui, text, size, p.text_muted, p.primary)
}

fn link_colored(
    ui: &mut Ui,
    text: &str,
    size: f32,
    color: Color32,
    hover: Color32,
) -> Response {
    let resp = ui
        .add(
            egui::Label::new(RichText::new(text).size(size).color(color))
                .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        let r = resp.rect;
        ui.painter().line_segment(
            [
                egui::pos2(r.left(), r.bottom() - 1.5),
                egui::pos2(r.right(), r.bottom() - 1.5),
            ],
            Stroke::new(1.0_f32, hover),
        );
    }
    resp
}

/// 联想输入框：输入时在下方浮出候选，点选即回填，未点选则保留用户原文。
///
/// 匹配用「包含」而不是前缀——中文输入法下用户常从中间几个字开始回忆，
/// 前缀匹配会漏掉大量正确结果。候选最多 8 条，避免浮层盖住整个表单。
/// 返回 `Some(值)` 表示这一帧用户点选了某一项（调用方据此刷新联动字段）。
pub fn autocomplete(
    ui: &mut Ui,
    id: &str,
    p: &Palette,
    text: &mut String,
    candidates: &[String],
    width: f32,
    hint: &str,
) -> Option<String> {
    let resp = ui.add_sized(
        [width, 26.0],
        egui::TextEdit::singleline(text)
            .hint_text(RichText::new(hint).size(12.0).color(p.text_muted))
            .margin(Margin::symmetric(8.0, 4.0)),
    );

    // 只有「正在输入」才弹候选：失焦或空串时一律不打扰
    if !resp.has_focus() {
        return None;
    }
    let q = text.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let matched: Vec<String> = candidates
        .iter()
        .filter(|c| c.to_lowercase().contains(&q))
        .take(8)
        .cloned()
        .collect();
    if matched.is_empty() {
        return None;
    }
    // 已经唯一命中且完全一致，就没有再选一次的必要
    if matched.len() == 1 && matched[0].to_lowercase() == q {
        return None;
    }

    let mut picked: Option<String> = None;
    egui::Area::new(egui::Id::new((id, "ac")))
        .order(egui::Order::Foreground)
        .fixed_pos(resp.rect.left_bottom() + egui::vec2(0.0, 4.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::none()
                .fill(p.surface)
                .stroke(Stroke::new(1.0_f32, p.border))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(6.0, 6.0))
                .show(ui, |ui| {
                    ui.set_width((width - 12.0).max(120.0));
                    for c in &matched {
                        if ui
                            .selectable_label(
                                false,
                                RichText::new(c).size(12.5).color(p.text),
                            )
                            .clicked()
                        {
                            picked = Some(c.clone());
                        }
                    }
                });
        });

    if let Some(v) = &picked {
        *text = v.clone();
        ui.memory_mut(|m| m.surrender_focus(resp.id));
    }
    picked
}

/// 表格「合计行」上方的分隔线。须在表格绘制完成后调用，`(left, right)` 取表格横向范围。
pub fn total_rule(ui: &Ui, p: &Palette, left: f32, right: f32, y: f32) {
    ui.painter().hline(
        left..=right,
        y,
        Stroke::new(1.0_f32, p.border_strong),
    );
}
