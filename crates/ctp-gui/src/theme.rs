//! 主题：浅色 / 深色两套配色与全局控件样式。
//!
//! 设计约定：
//! * 主色取自文档色系 `#1B5FA8`，浅色下用于主按钮、选中态与链接；
//! * 圆角统一 8px（卡片 12px），边框 1px，靠“底色分层 + 细边框”而不是重阴影区分层级；
//! * 文字三级：正文 / 次要 / 弱化，保证表格在浅深两套里都可读。

use eframe::egui::{self, Color32, FontId, Rounding, Stroke, TextStyle, Visuals};

/// 主题模式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeMode {
    Light,
    Dark,
}

impl ThemeMode {
    pub fn toggle(&mut self) {
        *self = match self {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        };
    }

    pub fn is_dark(self) -> bool {
        matches!(self, ThemeMode::Dark)
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Light => "浅色",
            ThemeMode::Dark => "深色",
        }
    }
}

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

/// 语义色板。
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub primary: Color32,
    pub primary_hover: Color32,
    pub primary_active: Color32,
    pub primary_soft: Color32,
    pub on_primary: Color32,
    pub success: Color32,
    pub success_soft: Color32,
    pub warn: Color32,
    pub warn_soft: Color32,
    pub danger: Color32,
    pub danger_hover: Color32,
    pub danger_active: Color32,
    pub danger_soft: Color32,
    pub shadow: Color32,
}

impl Palette {
    pub fn of(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self {
                bg: rgb(0xF4F6F9),
                surface: rgb(0xFFFFFF),
                surface_alt: rgb(0xF7F9FC),
                border: rgb(0xE3E9F0),
                border_strong: rgb(0xC7D2E0),
                text: rgb(0x16202C),
                text_muted: rgb(0x6B7A8C),
                primary: rgb(0x1B5FA8),
                primary_hover: rgb(0x2A72BF),
                primary_active: rgb(0x14497F),
                primary_soft: rgb(0xE7F0FA),
                on_primary: rgb(0xFFFFFF),
                success: rgb(0x1F8A4C),
                success_soft: rgb(0xE6F5EC),
                warn: rgb(0xB5761B),
                warn_soft: rgb(0xFCF3E2),
                danger: rgb(0xC23B2E),
                danger_hover: rgb(0xD14A3C),
                danger_active: rgb(0xA22E23),
                danger_soft: rgb(0xFBEAE7),
                shadow: Color32::from_black_alpha(18),
            },
            ThemeMode::Dark => Self {
                bg: rgb(0x0F151C),
                surface: rgb(0x161F29),
                surface_alt: rgb(0x1B2632),
                border: rgb(0x2A3846),
                border_strong: rgb(0x3D4E60),
                text: rgb(0xE6EDF5),
                text_muted: rgb(0x8FA1B3),
                primary: rgb(0x4E9BE6),
                primary_hover: rgb(0x67ADEF),
                primary_active: rgb(0x3C82C4),
                primary_soft: rgb(0x1C2E42),
                on_primary: rgb(0x0B1117),
                success: rgb(0x3FBF78),
                success_soft: rgb(0x16301F),
                warn: rgb(0xDFA53F),
                warn_soft: rgb(0x332912),
                danger: rgb(0xE8776A),
                danger_hover: rgb(0xF08A7E),
                danger_active: rgb(0xC85F53),
                danger_soft: rgb(0x35201D),
                shadow: Color32::from_black_alpha(120),
            },
        }
    }
}

/// 应用到全局 `Context`。切换主题时重新调用即可。
pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    let p = Palette::of(mode);
    let mut style = (*ctx.style()).clone();
    let mut v = if mode.is_dark() {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.surface_alt;
    v.faint_bg_color = p.surface_alt;
    v.override_text_color = Some(p.text);
    v.window_stroke = Stroke::new(1.0_f32, p.border);
    v.window_rounding = Rounding::same(12.0);
    v.menu_rounding = Rounding::same(10.0);
    v.window_shadow = egui::epaint::Shadow {
        offset: egui::vec2(0.0, 8.0),
        blur: 24.0,
        spread: 0.0,
        color: p.shadow,
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: egui::vec2(0.0, 4.0),
        blur: 14.0,
        spread: 0.0,
        color: p.shadow,
    };
    v.selection.bg_fill = p.primary_soft;
    v.selection.stroke = Stroke::new(1.0_f32, p.primary);

    // 四类控件状态：常态用次级底色、悬停上主色、按下加深，禁用降透明度。
    let mut w = v.widgets.noninteractive;
    w.bg_fill = p.surface;
    w.weak_bg_fill = p.surface;
    w.bg_stroke = Stroke::new(1.0_f32, p.border_strong);
    w.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.rounding = Rounding::same(8.0);
    w.expansion = 0.0;
    v.widgets.noninteractive = w;

    let mut w = v.widgets.inactive;
    w.bg_fill = p.surface;
    w.weak_bg_fill = p.surface;
    w.bg_stroke = Stroke::new(1.0_f32, p.border_strong);
    w.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.rounding = Rounding::same(8.0);
    w.expansion = 0.0;
    v.widgets.inactive = w;

    let mut w = v.widgets.hovered;
    w.bg_fill = p.primary_soft;
    w.weak_bg_fill = p.primary_soft;
    w.bg_stroke = Stroke::new(1.0_f32, p.primary);
    w.fg_stroke = Stroke::new(1.2_f32, p.primary);
    w.rounding = Rounding::same(8.0);
    w.expansion = 0.0;
    v.widgets.hovered = w;

    let mut w = v.widgets.active;
    w.bg_fill = p.primary_soft;
    w.weak_bg_fill = p.primary_soft;
    w.bg_stroke = Stroke::new(1.5_f32, p.primary_active);
    w.fg_stroke = Stroke::new(1.4_f32, p.primary_active);
    w.rounding = Rounding::same(8.0);
    w.expansion = 0.0;
    v.widgets.active = w;

    let mut w = v.widgets.open;
    w.bg_fill = p.primary_soft;
    w.weak_bg_fill = p.primary_soft;
    w.bg_stroke = Stroke::new(1.0_f32, p.primary);
    w.fg_stroke = Stroke::new(1.0_f32, p.primary);
    w.rounding = Rounding::same(8.0);
    v.widgets.open = w;

    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(22.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    ctx.set_style(style);
}
