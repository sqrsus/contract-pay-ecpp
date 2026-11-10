//! 通用视觉组件：卡片、KPI、徽标、进度条、按钮。
//!
//! 所有组件只依赖 `Palette`，因此浅色/深色两套主题下表现一致。

use crate::theme::Palette;
use eframe::egui::{self, Color32, Margin, Response, RichText, Rounding, Stroke, Ui, Vec2};

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
pub fn kpi(ui: &mut Ui, p: &Palette, width: f32, label: &str, value: &str, sub: &str, tone: Tone) {
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

/// 指定尺寸的次要按钮：登录页这类需要两个入口等宽并排的场景用。
pub fn ghost_button_sized(
    ui: &mut Ui,
    p: &Palette,
    text: &str,
    width: f32,
    height: f32,
) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(13.5).color(p.text))
            .fill(p.surface)
            .stroke(Stroke::new(1.0_f32, p.border))
            .rounding(Rounding::same(8.0))
            .min_size(Vec2::new(width, height)),
    )
}

/// 指定尺寸的主按钮（外观与 `primary_button` 一致，仅换成固定宽高）。
pub fn primary_button_sized(
    ui: &mut Ui,
    p: &Palette,
    text: &str,
    width: f32,
    height: f32,
    enabled: bool,
) -> Response {
    if !enabled {
        return ui.add(
            egui::Button::new(RichText::new(text).size(14.0).color(p.text_muted))
                .fill(p.surface_alt)
                .stroke(Stroke::new(1.0_f32, p.border))
                .rounding(Rounding::same(8.0))
                .min_size(Vec2::new(width, height)),
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
                .min_size(Vec2::new(width, height)),
        )
    })
    .inner
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

/// 定宽文本：超出宽度时默认只显示开头；鼠标悬停后自动横向滚动查看全文。
pub fn marquee(ui: &mut Ui, text: &str, width: f32, size: f32, color: Color32) -> Response {
    marquee_sense(ui, text, width, size, color, egui::Sense::hover())
}

/// 同 `marquee`，但可指定交互类型（例如 `Sense::click()` 用作链接）。
pub fn marquee_sense(
    ui: &mut Ui,
    text: &str,
    width: f32,
    size: f32,
    color: Color32,
    sense: egui::Sense,
) -> Response {
    marquee_sense_align(ui, text, width, size, color, egui::Align::LEFT, sense)
}

/// 同 `marquee_sense`，但对齐方式可指定（表格里右对齐的金额列用 `Align::RIGHT`）。
///
/// 放不下时悬停自动循环左移，**未悬停时仍从左侧开始**——右对齐列一旦超宽，
/// 先看到的是开头（合同名、事由这类字段的开头更有辨识度）。
pub fn marquee_sense_align(
    ui: &mut Ui,
    text: &str,
    width: f32,
    size: f32,
    color: Color32,
    align: egui::Align,
    sense: egui::Sense,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width.max(1.0), 18.0), sense);
    let font = egui::FontId::proportional(size);
    let galley = ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font, color));
    let text_w = galley.size().x;
    let gap = 28.0;
    let cycle = text_w + gap;

    let mut offset = 0.0_f32;
    if text_w > width && resp.hovered() {
        ui.ctx().request_repaint();
        let t = ui.input(|i| i.time) as f32;
        offset = (t * 32.0) % cycle;
    }

    let painter = ui.painter().with_clip_rect(rect);
    let y = rect.center().y - galley.size().y / 2.0;
    // 超宽或正在滚动时一律以左边缘为基准；只有「放得下」才按对齐方式定位
    let x = if text_w > width || offset > 0.0 {
        rect.left() - offset
    } else {
        match align {
            egui::Align::RIGHT => rect.right() - text_w,
            egui::Align::Center => rect.center().x - text_w / 2.0,
            _ => rect.left(),
        }
    };
    painter.galley(egui::pos2(x, y), galley.clone(), color);
    if offset > 0.0 {
        painter.galley(egui::pos2(rect.left() - offset + cycle, y), galley, color);
    }
    resp
}

/// 表格单元格文本：宽度取当前可用宽度，放不下时悬停自动横向滚动。
///
/// 列宽由调用方（TableBuilder 列配置或定宽 cell）决定，这里不再改宽度，
/// 因此「默认完整可见、超宽才滚动」与 R-54 的下拉口径一致。
pub fn cell_marquee(
    ui: &mut Ui,
    text: &str,
    size: f32,
    color: Color32,
    align: egui::Align,
) -> Response {
    let w = ui.available_width().max(1.0);
    marquee_sense_align(ui, text, w, size, color, align, egui::Sense::hover())
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
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, Rounding::same(0.5), p.border);
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
fn link_colored(ui: &mut Ui, text: &str, size: f32, color: Color32, hover: Color32) -> Response {
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

/// 联想弹层的触发方式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SuggestMode {
    /// 只有输入了内容才筛选展示；输入为空不弹层。
    TypedOnly,
    /// 输入为空时也展示全部已有条目（发包人沿用既有行为）。
    ShowAllOnEmpty,
}

/// 联想框的一帧结果。
pub struct AcOutcome {
    /// 用户点选了某一项时的回填值
    pub picked: Option<String>,
}

/// 联想输入框：输入时在下方浮出候选，点选即回填，未点选则保留用户原文。
///
/// 匹配用「包含」而不是前缀——中文输入法下用户常从中间几个字开始回忆，
/// 前缀匹配会漏掉大量正确结果。候选最多 8 条，避免浮层盖住整个表单。
///
/// 展示规则：
/// * `TypedOnly`：空输入不展示；输入后按内容筛选；无匹配时给一行「无匹配项」提示。
/// * `ShowAllOnEmpty`：空输入展示全部已有条目；输入后筛选；无匹配不弹层。
///
/// 返回 `Some(值)` 表示这一帧用户点选了某一项（调用方据此刷新联动字段）。
// 组件入参是 UI 状态与展示配置的扁平组合，调用点遍布各表单页。
#[allow(clippy::too_many_arguments)]
pub fn autocomplete(
    ui: &mut Ui,
    id: &str,
    p: &Palette,
    text: &mut String,
    candidates: &[String],
    width: f32,
    hint: &str,
    mode: SuggestMode,
) -> Option<String> {
    autocomplete_full(ui, id, p, text, candidates, width, hint, mode).picked
}

/// 同 `autocomplete`，额外回报「输入是否变化」与焦点状态。
// 与 autocomplete 共用同一组入参，保持签名一致。
#[allow(clippy::too_many_arguments)]
pub fn autocomplete_full(
    ui: &mut Ui,
    id: &str,
    p: &Palette,
    text: &mut String,
    candidates: &[String],
    width: f32,
    hint: &str,
    mode: SuggestMode,
) -> AcOutcome {
    autocomplete_impl(ui, id, p, text, candidates, width, hint, mode, None)
}

/// 同 `autocomplete`，但点选后写入的是 `pick(候选)` 的结果。
///
/// 项目候选要同时具备两种身份：列表里显示「编码 名称」方便按编码检索，
/// 但回填到输入框时只留名称（与新建合同时手输的名称一致）。
#[allow(clippy::too_many_arguments)]
pub fn autocomplete_pick(
    ui: &mut Ui,
    id: &str,
    p: &Palette,
    text: &mut String,
    candidates: &[String],
    width: f32,
    hint: &str,
    mode: SuggestMode,
    pick: &dyn Fn(&str) -> String,
) -> Option<String> {
    autocomplete_impl(ui, id, p, text, candidates, width, hint, mode, Some(pick)).picked
}

// 内部实现：多一个可选的「回填值换算」，其余入参与公开入口一一对应。
#[allow(clippy::too_many_arguments)]
fn autocomplete_impl(
    ui: &mut Ui,
    id: &str,
    p: &Palette,
    text: &mut String,
    candidates: &[String],
    width: f32,
    hint: &str,
    mode: SuggestMode,
    pick: Option<&dyn Fn(&str) -> String>,
) -> AcOutcome {
    let resp = ui.add_sized(
        [width, 26.0],
        egui::TextEdit::singleline(text)
            .hint_text(RichText::new(hint).size(12.0).color(p.text_muted))
            .margin(Margin::symmetric(8.0, 4.0)),
    );

    // 弹层显隐不能只看 `has_focus`：鼠标按在候选上时输入框会先失去焦点，
    // 若此时提前 return，候选列表当帧不再绘制，这次点击就永远丢失。
    // 因此用 memory 记住「展开」状态，并用上一帧弹层矩形判断点击是否落在外面。
    let open_id = egui::Id::new((id, "ac_open"));
    let rect_id = egui::Id::new((id, "ac_rect"));
    let last_rect: Option<egui::Rect> = ui
        .memory(|m| m.data.get_temp::<Option<egui::Rect>>(rect_id))
        .flatten();
    let mut open = ui.memory(|m| m.data.get_temp::<bool>(open_id).unwrap_or(false));

    if resp.has_focus() || resp.clicked() {
        open = true;
    }
    if ui.input(|i| i.pointer.any_pressed()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            let in_input = resp.rect.contains(pos);
            let in_popup = last_rect.is_some_and(|r| r.contains(pos));
            if !in_input && !in_popup {
                open = false;
            }
        }
    }

    let empty = text.trim().is_empty();

    // TypedOnly：输入为空时直接收起，不做任何展示。
    if empty && mode == SuggestMode::TypedOnly {
        open = false;
    }

    let matched = matched_candidates(candidates, text, mode);

    // 已输入但一个都没匹配上：给一行灰色提示，说明可继续手工输入新名称。
    // 发包人（ShowAllOnEmpty）保持原行为——无匹配时不弹层。
    let no_match_hint = mode == SuggestMode::TypedOnly && !empty && matched.is_empty();

    let mut picked: Option<String> = None;
    let mut popup_rect: Option<egui::Rect> = None;

    if open && (!matched.is_empty() || no_match_hint) {
        let area = egui::Area::new(egui::Id::new((id, "ac_area")))
            .order(egui::Order::Foreground)
            .fixed_pos(resp.rect.left_bottom() + egui::vec2(0.0, 4.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::none()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::symmetric(6.0, 6.0))
                    .show(ui, |ui| {
                        let row_w = (width - 12.0).max(120.0);
                        ui.set_width(row_w);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if no_match_hint {
                            ui.label(
                                RichText::new("无匹配项，可直接输入新名称")
                                    .size(11.5)
                                    .color(p.text_muted),
                            );
                        } else {
                            for c in &matched {
                                if suggestion_row(ui, p, c, row_w).clicked() {
                                    picked = Some(c.clone());
                                }
                            }
                        }
                    });
            });
        popup_rect = Some(area.response.rect);
    }

    let still_open = open && picked.is_none();
    ui.memory_mut(|m| {
        m.data.insert_temp(open_id, still_open);
        m.data.insert_temp(rect_id, popup_rect);
    });

    if let Some(v) = picked {
        let v = pick.map(|f| f(&v)).unwrap_or(v);
        *text = v.clone();
        ui.memory_mut(|m| m.surrender_focus(resp.id));
        return AcOutcome { picked: Some(v) };
    }
    AcOutcome { picked: None }
}

/// 候选上限：避免浮层盖住整个表单。
const MAX_SUGGESTIONS: usize = 8;

/// 纯函数：按输入过滤候选。
///
/// * 空输入：`TypedOnly` 返回空（不展示），`ShowAllOnEmpty` 返回前 8 条；
/// * 非空输入：忽略大小写、按「包含」匹配，最多 8 条；
/// * 无匹配：返回空，由调用方决定是否显示「无匹配项」提示。
fn matched_candidates(candidates: &[String], text: &str, mode: SuggestMode) -> Vec<String> {
    let q = text.trim().to_lowercase();
    if q.is_empty() {
        return match mode {
            SuggestMode::TypedOnly => Vec::new(),
            SuggestMode::ShowAllOnEmpty => {
                candidates.iter().take(MAX_SUGGESTIONS).cloned().collect()
            }
        };
    }
    candidates
        .iter()
        .filter(|c| c.to_lowercase().contains(&q))
        .take(MAX_SUGGESTIONS)
        .cloned()
        .collect()
}

/// 联想候选行：整行可点、悬停高亮，避免只有文字本身能点中。
fn suggestion_row(ui: &mut Ui, p: &Palette, text: &str, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, Rounding::same(5.0), p.primary_soft);
    }
    ui.painter().with_clip_rect(rect).text(
        egui::pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(12.5),
        p.text,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 合同下拉一次最多画多少行：几百份合同一次铺开既卡帧也看不清，超出靠筛选收敛。
const MAX_CONTRACT_ROWS: usize = 200;

/// 合同名称字号，收起态按钮与下拉行共用。
const NAME_SIZE: f32 = 12.5;
/// 行左内缩；选中行要让开主色竖条，多缩 4 px。
const ROW_LEFT_PAD: f32 = 8.0;
const ROW_LEFT_PAD_SELECTED: f32 = 12.0;
/// 名称与右侧承包单位列之间的最小间距。
const ROW_RIGHT_GAP: f32 = 6.0;
/// 同名时右侧承包单位列的宽度。
const DUP_ORG_W: f32 = 128.0;
/// 名称溢出时右侧 `›` 提示预留的宽度。
const OVERFLOW_HINT_W: f32 = 12.0;

/// 名称的排版宽度（与绘制同字体同字号）：判断是否需要横向滚动的唯一依据。
fn name_width(ui: &Ui, name: &str) -> f32 {
    ui.fonts(|f| {
        f.layout_no_wrap(
            name.to_owned(),
            egui::FontId::proportional(NAME_SIZE),
            Color32::WHITE,
        )
        .size()
        .x
    })
}

/// 按筛选词过滤合同：空格分隔多个词，**全部**命中才算（AND）。
///
/// R-54 起**只看合同名称**：编号与承包单位都不再参与匹配，
/// 与「下拉项只显示名称」保持一致——看得见什么就能搜什么。
/// 空筛选返回全部，下拉要能看到数据库里的所有合同（R-53）。
fn contract_hits<'a>(
    items: &'a [ctp_core::ledger::ContractSummary],
    filter: &str,
) -> Vec<&'a ctp_core::ledger::ContractSummary> {
    let words: Vec<String> = filter
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    items
        .iter()
        .filter(|s| {
            words
                .iter()
                .all(|w| s.contract_name.to_lowercase().contains(w.as_str()))
        })
        .collect()
}

/// 弹层内容宽度：按本屏最长的合同名称撑开，夹在「按钮宽度」与「屏幕可用宽度」之间。
///
/// 默认态是名称完整可见；只有达到屏幕上限后才出现横向滚动（R-54）。
fn popup_inner_width(longest_name: f32, right_w: f32, base: f32, cap: f32) -> f32 {
    let needed = ROW_LEFT_PAD_SELECTED + longest_name + ROW_RIGHT_GAP + right_w + OVERFLOW_HINT_W;
    needed.clamp(base, cap.max(base))
}

/// 横向滚动的记忆键：按用途 + 合同 id 分桶，换一份合同自然回到最左。
fn hscroll_id(widget: &str, scope: &str, key: &str) -> egui::Id {
    egui::Id::new((widget, scope, key))
}

/// 处理「Shift + 滚轮」的横向滚动（egui 已把 Shift+纵向滚轮折算成横向 delta），
/// 返回本帧的偏移量。只有 `content_w > view_w` 时才可能非 0。
fn hscroll_offset(ui: &Ui, key: egui::Id, hovered: bool, content_w: f32, view_w: f32) -> f32 {
    let max_off = (content_w - view_w).max(0.0);
    let mut off = ui
        .memory(|m| m.data.get_temp::<f32>(key))
        .unwrap_or(0.0)
        .clamp(0.0, max_off);
    if max_off <= 0.0 {
        ui.memory_mut(|m| m.data.insert_temp(key, 0.0));
        return 0.0;
    }
    if hovered {
        let dx = ui.input(|i| i.smooth_scroll_delta.x);
        if dx != 0.0 {
            off = (off - dx).clamp(0.0, max_off);
            // 吃掉横向分量，避免同一帧里再被别的滚动区消费一次
            ui.ctx().input_mut(|i| i.smooth_scroll_delta.x = 0.0);
        }
    }
    ui.memory_mut(|m| m.data.insert_temp(key, off));
    off
}

/// 在 `area` 内画合同名称：放不下时按住 Shift 滚轮左右滚动查看，
/// 右侧常驻 `›` 提示还有内容，滚到末尾提示消失（R-54）。
fn scrollable_name(
    ui: &Ui,
    p: &Palette,
    text: &str,
    area: egui::Rect,
    key: egui::Id,
    hovered: bool,
    color: Color32,
) {
    let galley = ui.fonts(|f| {
        f.layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(NAME_SIZE),
            color,
        )
    });
    let text_w = galley.size().x;
    let overflow = text_w > area.width() + 0.5;
    // 溢出时右侧让出 12 px 给 `›`，避免提示压在字上
    let view_w = if overflow {
        (area.width() - OVERFLOW_HINT_W).max(1.0)
    } else {
        area.width()
    };
    let off = hscroll_offset(ui, key, hovered && overflow, text_w, view_w);
    let view = egui::Rect::from_min_max(area.min, egui::pos2(area.left() + view_w, area.bottom()));
    let y = area.center().y - galley.size().y / 2.0;
    ui.painter()
        .with_clip_rect(view)
        .galley(egui::pos2(view.left() - off, y), galley, color);
    if overflow && off < text_w - view_w - 0.5 {
        ui.painter().text(
            egui::pos2(area.right(), area.center().y),
            egui::Align2::RIGHT_CENTER,
            "›",
            egui::FontId::proportional(14.0),
            p.text_muted,
        );
    }
}

/// 可搜索的合同下拉框：按钮显示当前合同，展开后列出全部合同，顶部可输入筛选。
///
/// 与联想框的关键差异是「真值」：这里选中的是**合同 id**，输入框只是筛选词，
/// 因此不存在「文字对不上合同」这种中间态——选谁就是谁（R-53）。
/// `open` 由调用方持有，截图与回归核对时可直接置为展开。
#[allow(clippy::too_many_arguments)]
pub fn contract_select(
    ui: &mut Ui,
    id: &str,
    p: &Palette,
    items: &[ctp_core::ledger::ContractSummary],
    selected: Option<&str>,
    filter: &mut String,
    open: &mut bool,
    width: f32,
) -> Option<String> {
    let current = selected.and_then(|sid| items.iter().find(|s| s.contract_id == sid));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 26.0), egui::Sense::click());
    ui.painter().rect(
        rect,
        Rounding::same(6.0),
        if resp.hovered() || *open {
            p.surface_alt
        } else {
            p.surface
        },
        Stroke::new(1.0_f32, if *open { p.primary } else { p.border }),
    );
    // 收起态只显示名称（R-54）：放不下时同样按住 Shift 滚轮左右看，不再拼编号
    let (label, label_color) = match current {
        Some(s) => (s.contract_name.as_str(), p.text),
        None => ("请选择合同", p.text_muted),
    };
    // 文字区右侧给箭头留 24 px，名称再长也不压缩箭头
    let label_clip = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 8.0, rect.top()),
        egui::pos2(rect.right() - 24.0, rect.bottom()),
    );
    scrollable_name(
        ui,
        p,
        label,
        label_clip,
        hscroll_id(id, "cs_btn", selected.unwrap_or("-")),
        resp.hovered(),
        label_color,
    );
    // 字体里没有 ▾ 的字形（会画成空心方块），下拉箭头直接用两条线画
    let arrow = egui::pos2(rect.right() - 14.0, rect.center().y - 2.0);
    let arrow_stroke = Stroke::new(1.4_f32, p.text_muted);
    ui.painter().line_segment(
        [arrow + egui::vec2(-4.0, 0.0), arrow + egui::vec2(0.0, 4.0)],
        arrow_stroke,
    );
    ui.painter().line_segment(
        [arrow + egui::vec2(0.0, 4.0), arrow + egui::vec2(4.0, 0.0)],
        arrow_stroke,
    );
    // 名称超出按钮宽度时给出滚动提示，正常情况下不打扰
    let resp = if name_width(ui, label) > label_clip.width() {
        resp.on_hover_text("按住 Shift + 滚轮左右查看完整名称")
    } else {
        resp
    };

    if resp.clicked() {
        *open = !*open;
    }

    let prev_open_id = egui::Id::new((id, "cs_prev"));
    let rect_id = egui::Id::new((id, "cs_rect"));
    let hl_id = egui::Id::new((id, "cs_hl"));
    let prev_open = ui
        .memory(|m| m.data.get_temp::<bool>(prev_open_id))
        .unwrap_or(false);
    let just_opened = *open && !prev_open;

    // 点击弹层外面收起。用上一帧的弹层矩形判断，避免点列表项时当帧已被销毁。
    let last_rect: Option<egui::Rect> = ui
        .memory(|m| m.data.get_temp::<Option<egui::Rect>>(rect_id))
        .flatten();
    if *open && ui.input(|i| i.pointer.any_pressed()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            let in_button = rect.contains(pos);
            let in_popup = last_rect.is_some_and(|r| r.contains(pos));
            if !in_button && !in_popup {
                *open = false;
            }
        }
    }
    if *open && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        *open = false;
    }

    // 上下键与回车先在筛选框之前吃掉，否则单行文本框会把方向键当成光标移动。
    let (key_up, key_down, key_enter) = if *open {
        (
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)),
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)),
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)),
        )
    } else {
        (false, false, false)
    };

    // 筛选词不随展开重置：用户上次筛到一半就关掉时，再点开还在原处；
    // 选中合同后由调用方清空，所以正常流程下次展开仍是全部合同。

    let mut picked: Option<String> = None;

    if *open {
        let mut hl: usize = ui.memory(|m| m.data.get_temp::<usize>(hl_id)).unwrap_or(0);
        let hits = contract_hits(items, filter);
        // 同名时光看名称分不清，右侧才补承包单位；这里同时决定整列预留的宽度
        let mut name_count: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for s in &hits {
            *name_count.entry(s.contract_name.as_str()).or_insert(0) += 1;
        }
        let right_w = if name_count.values().any(|&n| n > 1) {
            DUP_ORG_W
        } else {
            0.0
        };

        // 弹层默认按本屏最长的名称撑开，名称完整可见；达到窗口上限才退化为横向滚动
        let longest = hits
            .iter()
            .map(|s| name_width(ui, &s.contract_name))
            .fold(0.0_f32, f32::max);
        let base_inner = (width - 16.0).max(200.0);
        let cap = (ui.ctx().screen_rect().width() - 64.0).max(base_inner);
        let inner_w = popup_inner_width(longest, right_w, base_inner, cap);
        let name_cut = ROW_LEFT_PAD_SELECTED + longest + ROW_RIGHT_GAP + right_w + OVERFLOW_HINT_W
            > inner_w + 0.5;

        // 弹层比按钮宽时向右撑会被窗口切掉，先把整体位置往左挪
        let total_w = inner_w + 18.0;
        let screen = ui.ctx().screen_rect();
        let left = rect
            .left()
            .min((screen.right() - total_w - 12.0).max(8.0))
            .max(8.0);

        let area = egui::Area::new(egui::Id::new((id, "cs_area")))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(left, rect.bottom() + 4.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::none()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0_f32, p.border_strong))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::symmetric(8.0, 8.0))
                    .show(ui, |ui| {
                        ui.set_width(inner_w);
                        let fresp = ui.add_sized(
                            [inner_w, 26.0],
                            egui::TextEdit::singleline(filter)
                                .hint_text(
                                    RichText::new("输入合同名称筛选")
                                        .size(11.5)
                                        .color(p.text_muted),
                                )
                                .margin(Margin::symmetric(8.0, 4.0)),
                        );
                        if just_opened {
                            fresp.request_focus();
                        }

                        ui.add_space(6.0);
                        let rule_y = ui.cursor().top();
                        let l = ui.min_rect().left();
                        ui.painter()
                            .hline(l..=(l + inner_w), rule_y, Stroke::new(1.0_f32, p.border));
                        ui.add_space(6.0);

                        if just_opened {
                            hl = hits
                                .iter()
                                .position(|s| Some(s.contract_id.as_str()) == selected)
                                .unwrap_or(0);
                        }
                        if hl >= hits.len() {
                            hl = hits.len().saturating_sub(1);
                        }
                        if key_down {
                            hl = (hl + 1).min(hits.len().saturating_sub(1));
                        }
                        if key_up {
                            hl = hl.saturating_sub(1);
                        }

                        let shown = hits.len().min(MAX_CONTRACT_ROWS);
                        if hits.is_empty() {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("没有匹配的合同，换个关键字试试")
                                    .size(11.5)
                                    .color(p.text_muted),
                            );
                            ui.add_space(6.0);
                        } else {
                            egui::ScrollArea::vertical()
                                .max_height(300.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    ui.set_width(inner_w);
                                    for (i, s) in hits.iter().take(shown).enumerate() {
                                        let dup = name_count
                                            .get(s.contract_name.as_str())
                                            .copied()
                                            .unwrap_or(0)
                                            > 1;
                                        let right = if dup {
                                            Some(s.contractor_name.as_str())
                                        } else {
                                            None
                                        };
                                        let row = contract_row(
                                            ui,
                                            p,
                                            s,
                                            right,
                                            selected == Some(s.contract_id.as_str()),
                                            i == hl,
                                            inner_w,
                                            id,
                                        );
                                        if row.hovered() {
                                            hl = i;
                                        }
                                        if row.clicked() {
                                            picked = Some(s.contract_id.clone());
                                        }
                                    }
                                });
                            if key_enter {
                                picked = hits.get(hl).map(|s| s.contract_id.clone());
                            }
                        }

                        ui.add_space(4.0);
                        let total = items.len();
                        let tip = if hits.len() > shown {
                            format!(
                                "共 {total} 份 · 匹配 {} 份，仅显示前 {MAX_CONTRACT_ROWS} 份，请继续输入缩小范围",
                                hits.len()
                            )
                        } else if filter.trim().is_empty() {
                            format!("共 {total} 份合同")
                        } else {
                            format!("匹配 {} / 共 {total} 份", hits.len())
                        };
                        let tip = if name_cut {
                            format!("{tip} · 名称过长时按住 Shift 滚轮左右查看")
                        } else {
                            tip
                        };
                        ui.label(RichText::new(tip).size(10.5).color(p.text_muted));
                    });
            });
        let popup_rect = Some(area.response.rect);
        ui.memory_mut(|m| {
            m.data.insert_temp(hl_id, hl);
            m.data.insert_temp(rect_id, popup_rect);
            m.data.insert_temp(prev_open_id, *open);
        });
    } else {
        ui.memory_mut(|m| {
            m.data.insert_temp(rect_id, None::<egui::Rect>);
            m.data.insert_temp(prev_open_id, false);
        });
    }

    if picked.is_some() {
        *open = false;
    }
    picked
}

/// 下拉里的一行：整行可点，悬停/高亮有底色，当前选中项左侧带主色竖条。
///
/// 只画合同名称（R-54）；名称超出可用宽度时按 Shift+滚轮横向滚动，
/// 同名时右侧补承包单位小字，否则两行一模一样无从分辨。
#[allow(clippy::too_many_arguments)]
fn contract_row(
    ui: &mut Ui,
    p: &Palette,
    s: &ctp_core::ledger::ContractSummary,
    right: Option<&str>,
    selected: bool,
    highlight: bool,
    width: f32,
    widget: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::click());
    if selected {
        ui.painter()
            .rect_filled(rect, Rounding::same(5.0), p.primary_soft);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + 2.0, rect.top() + 5.0),
                egui::vec2(2.5, rect.height() - 10.0),
            ),
            Rounding::same(1.0),
            p.primary,
        );
    } else if highlight || resp.hovered() {
        ui.painter()
            .rect_filled(rect, Rounding::same(5.0), p.surface_alt);
    }

    let left_pad = if selected {
        ROW_LEFT_PAD_SELECTED
    } else {
        ROW_LEFT_PAD
    };
    let right_w = if right.is_some() { DUP_ORG_W } else { 0.0 };
    let name_area = egui::Rect::from_min_max(
        egui::pos2(rect.left() + left_pad, rect.top()),
        egui::pos2(rect.right() - right_w - ROW_RIGHT_GAP, rect.bottom()),
    );
    scrollable_name(
        ui,
        p,
        &s.contract_name,
        name_area,
        hscroll_id(widget, "cs_row", &s.contract_id),
        resp.hovered(),
        p.text,
    );
    if let Some(t) = right {
        let clip = egui::Rect::from_min_max(
            egui::pos2(rect.right() - right_w, rect.top()),
            egui::pos2(rect.right() - 6.0, rect.bottom()),
        );
        ui.painter().with_clip_rect(clip).text(
            egui::pos2(rect.right() - 6.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            t,
            egui::FontId::proportional(10.5),
            p.text_muted,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 日期输入框：点击弹出月历，选中即回填 `YYYY-MM-DD`；也可直接键盘输入。
///
/// 与联想框同理，弹层用 memory 维持——点日历格子时输入框会先失焦，
/// 若依赖 `has_focus` 就会当帧收起、点击丢失。
pub fn date_input(ui: &mut Ui, id: &str, p: &Palette, text: &mut String, width: f32) {
    let resp = ui.add_sized(
        [width, 26.0],
        egui::TextEdit::singleline(text)
            .hint_text(RichText::new("YYYY-MM-DD").size(12.0).color(p.text_muted))
            .margin(Margin::symmetric(8.0, 4.0)),
    );

    let open_id = egui::Id::new((id, "dt_open"));
    let rect_id = egui::Id::new((id, "dt_rect"));
    let ym_id = egui::Id::new((id, "dt_ym"));

    let last_rect: Option<egui::Rect> = ui
        .memory(|m| m.data.get_temp::<Option<egui::Rect>>(rect_id))
        .flatten();
    let mut open = ui.memory(|m| m.data.get_temp::<bool>(open_id).unwrap_or(false));

    if resp.clicked() {
        open = true;
        // 每次点开都按当前文本重新定位月份，避免沿用上次浏览到的月份
        if let Some((y, m)) = parse_ym(text) {
            ui.memory_mut(|mem| mem.data.insert_temp(ym_id, (y, m)));
        }
    }
    // 键盘输入时不挡着日历
    if resp.changed() {
        open = false;
    }
    if ui.input(|i| i.pointer.any_pressed()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            let in_input = resp.rect.contains(pos);
            let in_popup = last_rect.is_some_and(|r| r.contains(pos));
            if !in_input && !in_popup {
                open = false;
            }
        }
    }

    let mut picked: Option<String> = None;
    let mut cleared = false;
    let mut popup_rect: Option<egui::Rect> = None;

    if open {
        let today = today_parts();
        let (mut y, mut m) = ui
            .memory(|mem| mem.data.get_temp::<(i32, u32)>(ym_id))
            .or_else(|| parse_ym(text))
            .unwrap_or((today.0, today.1));

        let mut nav = 0i32;
        let area = egui::Area::new(egui::Id::new((id, "dt_area")))
            .order(egui::Order::Foreground)
            .fixed_pos(resp.rect.left_bottom() + egui::vec2(0.0, 4.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::none()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .rounding(Rounding::same(10.0))
                    .inner_margin(Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.set_width(232.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_sized(
                                    [26.0, 24.0],
                                    egui::Button::new(RichText::new("‹").size(16.0).color(p.text))
                                        .frame(false),
                                )
                                .clicked()
                            {
                                nav = -1;
                            }
                            ui.label(
                                RichText::new(format!("{y} 年 {m} 月"))
                                    .size(13.0)
                                    .strong()
                                    .color(p.text),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [26.0, 24.0],
                                            egui::Button::new(
                                                RichText::new("›").size(16.0).color(p.text),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        nav = 1;
                                    }
                                },
                            );
                        });
                        ui.add_space(4.0);
                        egui::Grid::new(egui::Id::new((id, "dt_grid")))
                            .num_columns(7)
                            .spacing([2.0, 2.0])
                            .show(ui, |ui| {
                                for (i, w) in ["一", "二", "三", "四", "五", "六", "日"]
                                    .iter()
                                    .enumerate()
                                {
                                    ui.label(RichText::new(*w).size(11.0).color(p.text_muted));
                                    if i == 6 {
                                        ui.end_row();
                                    }
                                }
                                let first = weekday_monday0(y, m, 1);
                                let days = days_in_month(y, m);
                                let mut col = 0u32;
                                for _ in 0..first {
                                    ui.label("");
                                    col += 1;
                                }
                                for d in 1..=days {
                                    let iso = format!("{y:04}-{m:02}-{d:02}");
                                    let selected = text.as_str() == iso.as_str();
                                    let is_today = today == (y, m, d);
                                    if day_cell(ui, p, d, selected, is_today).clicked() {
                                        picked = Some(iso);
                                    }
                                    col += 1;
                                    if col == 7 {
                                        ui.end_row();
                                        col = 0;
                                    }
                                }
                                if col != 0 {
                                    ui.end_row();
                                }
                            });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("今天").size(11.5).color(p.primary),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                picked =
                                    Some(format!("{:04}-{:02}-{:02}", today.0, today.1, today.2));
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("清空").size(11.5).color(p.text_muted),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                cleared = true;
                                *text = String::new();
                            }
                        });
                    });
            });
        popup_rect = Some(area.response.rect);

        if nav != 0 {
            let mut nm = m as i32 + nav;
            while nm < 1 {
                nm += 12;
                y -= 1;
            }
            while nm > 12 {
                nm -= 12;
                y += 1;
            }
            m = nm as u32;
            ui.memory_mut(|mem| mem.data.insert_temp(ym_id, (y, m)));
        }
    }

    if let Some(v) = picked {
        *text = v;
        open = false;
    }
    if cleared {
        open = false;
    }
    ui.memory_mut(|m| {
        m.data.insert_temp(open_id, open);
        m.data.insert_temp(rect_id, popup_rect);
    });
}

/// 日历格子：整格可点，选中实心主色、今日浅主色、悬停浅主色。
fn day_cell(ui: &mut Ui, p: &Palette, day: u32, selected: bool, is_today: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 24.0), egui::Sense::click());
    let fill = if selected {
        p.primary
    } else if resp.hovered() || is_today {
        p.primary_soft
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, Rounding::same(6.0), fill);
    }
    let fg = if selected { p.on_primary } else { p.text };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        day.to_string(),
        egui::FontId::proportional(12.0),
        fg,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn parse_ym(s: &str) -> Option<(i32, u32)> {
    let b = s.as_bytes();
    if b.len() < 7 || b[4] != b'-' {
        return None;
    }
    let y: i32 = s.get(0..4)?.parse().ok()?;
    let m: u32 = s.get(5..7)?.parse().ok()?;
    if (1..=12).contains(&m) {
        Some((y, m))
    } else {
        None
    }
}

fn today_parts() -> (i32, u32, u32) {
    // R-68（C6）：日历高亮的「今天」按东八区，与业务默认日期同口径
    let t = ctp_core::date::today_local_iso();
    let y = t.get(0..4).and_then(|x| x.parse().ok()).unwrap_or(2026);
    let m = t.get(5..7).and_then(|x| x.parse().ok()).unwrap_or(1);
    let d = t.get(8..10).and_then(|x| x.parse().ok()).unwrap_or(1);
    (y, m, d)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// 周一为 0 的星期序号（Sakamoto 公式）。
fn weekday_monday0(y: i32, m: u32, d: u32) -> u32 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let yy = if m < 3 { y - 1 } else { y };
    let sun0 =
        (yy + yy / 4 - yy / 100 + yy / 400 + T[(m - 1) as usize] + d as i32).rem_euclid(7) as u32;
    (sun0 + 6) % 7
}

/// 表格「合计行」上方的分隔线。须在表格绘制完成后调用，`(left, right)` 取表格横向范围。
pub fn total_rule(ui: &Ui, p: &Palette, left: f32, right: f32, y: f32) {
    ui.painter()
        .hline(left..=right, y, Stroke::new(1.0_f32, p.border_strong));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 样例() -> Vec<String> {
        vec![
            "XATJ 某市政道路改扩建工程".to_string(),
            "某建工集团有限公司".to_string(),
            "某工程监理有限公司".to_string(),
        ]
    }

    #[test]
    fn 联想_空输入不展示() {
        assert!(matched_candidates(&样例(), "", SuggestMode::TypedOnly).is_empty());
        assert!(matched_candidates(&样例(), "   ", SuggestMode::TypedOnly).is_empty());
    }

    #[test]
    fn 联想_按包含匹配且忽略大小写() {
        let hit = matched_candidates(&样例(), "xatj", SuggestMode::TypedOnly);
        assert_eq!(hit.len(), 1);
        assert!(hit[0].contains("XATJ"));

        let hit2 = matched_candidates(&样例(), "某建工", SuggestMode::TypedOnly);
        assert_eq!(hit2, vec!["某建工集团有限公司".to_string()]);
    }

    #[test]
    fn 联想_无匹配返回空() {
        assert!(matched_candidates(&样例(), "不存在的名称", SuggestMode::TypedOnly).is_empty());
    }

    #[test]
    fn 联想_发包人空输入仍展示全部() {
        assert_eq!(
            matched_candidates(&样例(), "", SuggestMode::ShowAllOnEmpty).len(),
            3
        );
    }

    fn 合同(
        id: &str,
        no: &str,
        name: &str,
        contractor: &str,
    ) -> ctp_core::ledger::ContractSummary {
        use ctp_core::money::Money;
        use ctp_core::rate::Rate;
        ctp_core::ledger::ContractSummary {
            contract_id: id.into(),
            project_id: "P".into(),
            contract_no: no.into(),
            contract_name: name.into(),
            status: "PERFORMING".into(),
            contractor_name: contractor.into(),
            contract_type: "CONSTRUCT".into(),
            contract_category_code: "GC".into(),
            category_code: None,
            category_name: None,
            total_amount: Money::from_cent(1_000),
            audit_price: None,
            is_audited: false,
            base_amount: Money::from_cent(1_000),
            audit_reduction: Money::ZERO,
            paid_amount: Money::ZERO,
            inflight_amount: Money::ZERO,
            occupied_amount: Money::ZERO,
            available_amount: Money::from_cent(1_000),
            pay_rate: Rate::from_bp(0),
            is_inverted: false,
            planned_end: None,
            duration_days: 0,
            delivery_date: None,
        }
    }

    fn 编号们(hits: &[&ctp_core::ledger::ContractSummary]) -> Vec<String> {
        hits.iter().map(|s| s.contract_no.clone()).collect()
    }

    /// R-53：空筛选必须列出全部合同，否则「涵盖数据库全部合同」这条要求落空。
    #[test]
    fn 合同下拉_空筛选列出全部() {
        let list = vec![
            合同(
                "C1",
                "HT-2026-001",
                "道路工程（一标段）施工总承包",
                "某建工集团",
            ),
            合同("C2", "HT-2026-002", "道路工程施工监理服务", "某监理公司"),
        ];
        assert_eq!(
            编号们(&contract_hits(&list, "")),
            ["HT-2026-001", "HT-2026-002"]
        );
        assert_eq!(
            编号们(&contract_hits(&list, "   ")),
            ["HT-2026-001", "HT-2026-002"]
        );
    }

    /// R-54：筛选只看合同名称——编号与承包单位都不再参与，
    /// 保证「看得见什么就能搜什么」，不出现搜到了却看不出原因的命中。
    #[test]
    fn 合同下拉_只按名称筛选() {
        let list = vec![
            合同(
                "C1",
                "HT-2026-001",
                "道路工程（一标段）施工总承包",
                "某建工集团",
            ),
            合同("C2", "HT-2026-002", "道路工程施工监理服务", "某监理公司"),
            合同("C3", "HT-2026-003", "沥青混合料采购", "某材料公司"),
        ];
        assert_eq!(
            编号们(&contract_hits(&list, "道路")),
            ["HT-2026-001", "HT-2026-002"]
        );
        // 多词 AND：都落在名称上，只剩监理合同
        assert_eq!(编号们(&contract_hits(&list, "道路 监理")), ["HT-2026-002"]);
        // 系统编号、承包单位都不再命中
        assert!(contract_hits(&list, "HT-2026-001").is_empty());
        assert!(contract_hits(&list, "某材料公司").is_empty());
        assert!(contract_hits(&list, "不存在的关键字").is_empty());
    }

    /// 名称里带外部标段号（真实库 HT-2026-005 的情形）仍按名称原文命中；
    /// 系统编号本身不再参与检索，半截编号也不会再「唯一命中就替你选」。
    #[test]
    fn 合同下拉_名称里的编号串按名称命中() {
        let list = vec![
            合同(
                "C5",
                "HT-2026-005",
                "2022年宿城区农村公路提档升级工程SC-NLTD-2022-SG1",
                "某路桥",
            ),
            合同(
                "C1",
                "HT-2026-001",
                "道路工程（一标段）施工总承包",
                "某建工集团",
            ),
        ];
        assert_eq!(编号们(&contract_hits(&list, "SC-NLTD")), ["HT-2026-005"]);
        assert!(contract_hits(&list, "HT-2026-001").is_empty());
        assert!(contract_hits(&list, "HT-2026-00").is_empty());
    }

    /// 弹层宽度：默认按最长名称撑开（名称完整可见），到屏幕上限才停住，
    /// 停住之后才轮到横向滚动。
    #[test]
    fn 合同下拉_弹层宽度按名称自适应且不超上限() {
        let base = 356.0;
        let cap = 1200.0;
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        // 短名称：保持按钮宽度，不跟着缩
        assert!(close(popup_inner_width(80.0, 0.0, base, cap), base));
        // 长名称：撑到刚好放下（左内缩 + 名称 + 间距 + 溢出提示）
        let wide = popup_inner_width(600.0, 0.0, base, cap);
        assert!(wide > 600.0 && wide < 660.0, "实际 {wide}");
        // 超过屏幕上限：停在 cap，剩余内容靠横向滚动
        assert!(close(popup_inner_width(4000.0, 0.0, base, cap), cap));
        // 同名时要额外给右侧承包单位列留位
        assert!(
            popup_inner_width(300.0, DUP_ORG_W, base, cap)
                > popup_inner_width(300.0, 0.0, base, cap)
        );
    }
}
