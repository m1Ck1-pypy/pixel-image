//! Retro widgets drawn directly into the software canvas.

use crate::UiState;
use crate::canvas::Canvas;
use crate::color::{Color, darken, lighten};
use crate::font::Font;
use crate::icons;
use crate::rect::Rect;
use crate::theme::{self, Metrics};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonState {
    Normal,
    Hover,
    Pressed,
    Disabled,
}

/// Colour family of a button. The kit has a single accent (blue); red is
/// reserved for the generate action by product decision, and secondary buttons
/// are neutral so a screen never carries too many accented controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonTone {
    Primary,
    Danger,
    Secondary,
}

/// White card with a hard shadow, a 1px outline and a light top bevel.
pub fn panel(canvas: &mut Canvas, rect: Rect) {
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    canvas.fill_rect(
        rect.x + theme::SHADOW,
        rect.y + theme::SHADOW,
        rect.w,
        rect.h,
        theme::BLACK,
    );
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, theme::PANEL);
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, theme::BLACK);
    canvas.hline(rect.x + 1, rect.y + 1, rect.w - 2, theme::WHITE);
}

/// Blue title strip of the application card.
pub fn window_header(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    title: &str,
    right: &str,
    metrics: Metrics,
) {
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, theme::BLUE);
    canvas.hline(rect.x, rect.y, rect.w, theme::BLUE_LIGHT);
    canvas.hline(rect.x, rect.bottom() - 1, rect.w, theme::BLACK);
    let title_y = rect.y + (rect.h - Font::glyph_height(metrics.title)) / 2;
    font.draw(
        canvas,
        rect.x + 14,
        title_y,
        title,
        theme::WHITE,
        metrics.title,
    );
    if !right.is_empty() {
        let y = rect.y + (rect.h - Font::glyph_height(metrics.small)) / 2;
        font.draw_right(
            canvas,
            rect.right() - 14,
            y,
            right,
            theme::WHITE,
            metrics.small,
        );
    }
}

/// Primary blue button. Returns true when it was clicked in this frame.
pub fn button(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    label: &str,
    enabled: bool,
    ui: &mut UiState,
    text_scale: i32,
) -> bool {
    button_tone(
        canvas,
        font,
        rect,
        label,
        enabled,
        ui,
        text_scale,
        ButtonTone::Primary,
    )
}

/// Button with an explicit colour family.
#[allow(clippy::too_many_arguments)]
pub fn button_tone(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    label: &str,
    enabled: bool,
    ui: &mut UiState,
    text_scale: i32,
    tone: ButtonTone,
) -> bool {
    let hovered = enabled && ui.hovered(rect);
    let pressed = hovered && ui.mouse_down;
    let state = if !enabled {
        ButtonState::Disabled
    } else if pressed {
        ButtonState::Pressed
    } else if hovered {
        ButtonState::Hover
    } else {
        ButtonState::Normal
    };
    draw_button(canvas, font, rect, label, state, text_scale, tone);
    enabled && ui.clicked(rect)
}

pub fn draw_button(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    label: &str,
    state: ButtonState,
    text_scale: i32,
    tone: ButtonTone,
) {
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    let (_, ink) = draw_button_frame(canvas, rect, state, tone);
    let scale = fit_scale(font, label, rect.w - theme::BUTTON_PAD, text_scale);
    let sink = if state == ButtonState::Pressed { 1 } else { 0 };
    let y = rect.y + (rect.h - Font::glyph_height(scale)) / 2 + sink;
    font.draw_centered(canvas, rect.center_x(), y, label, ink, scale);
}

/// Icon-only button: the [`button_tone`] frame with a 16x16 bitmap glyph in
/// place of a label. The caller owns the tooltip that names the action.
pub fn icon_button(
    canvas: &mut Canvas,
    rect: Rect,
    icon: &icons::Bitmap,
    enabled: bool,
    ui: &mut UiState,
    tone: ButtonTone,
) -> bool {
    if rect.w <= 0 || rect.h <= 0 {
        return false;
    }
    let hovered = enabled && ui.hovered(rect);
    let pressed = hovered && ui.mouse_down;
    let state = if !enabled {
        ButtonState::Disabled
    } else if pressed {
        ButtonState::Pressed
    } else if hovered {
        ButtonState::Hover
    } else {
        ButtonState::Normal
    };
    let (_, ink) = draw_button_frame(canvas, rect, state, tone);
    let sink = if state == ButtonState::Pressed { 1 } else { 0 };
    icons::glyph(
        canvas,
        Rect::new(rect.x, rect.y + sink, rect.w, rect.h),
        1,
        ink,
        icon,
    );
    enabled && ui.clicked(rect)
}

/// Frame shared by text and icon buttons. The returned fill/ink pair keeps the
/// glyph drawn on top at the same contrast as a label.
fn draw_button_frame(
    canvas: &mut Canvas,
    rect: Rect,
    state: ButtonState,
    tone: ButtonTone,
) -> (Color, Color) {
    let (fill, ink) = match (tone, state) {
        (_, ButtonState::Disabled) => (theme::DISABLED, theme::DISABLED_TEXT),
        (ButtonTone::Primary, ButtonState::Normal) => (theme::BLUE, theme::WHITE),
        (ButtonTone::Primary, ButtonState::Hover) => (theme::BLUE_LIGHT, theme::WHITE),
        (ButtonTone::Primary, ButtonState::Pressed) => (theme::BLUE_DARK, theme::WHITE),
        (ButtonTone::Danger, ButtonState::Normal) => (theme::RED, theme::WHITE),
        (ButtonTone::Danger, ButtonState::Hover) => (theme::RED_LIGHT, theme::WHITE),
        (ButtonTone::Danger, ButtonState::Pressed) => (theme::RED_DARK, theme::WHITE),
        (ButtonTone::Secondary, ButtonState::Normal) => (theme::PANEL_ALT, theme::TEXT),
        (ButtonTone::Secondary, ButtonState::Hover) => (theme::PANEL_HOVER, theme::TEXT),
        (ButtonTone::Secondary, ButtonState::Pressed) => (darken(theme::PANEL_ALT, 10), theme::TEXT),
    };
    canvas.fill_rect(rect.x + 2, rect.y + 2, rect.w, rect.h, theme::BLACK);
    canvas.fill_round_rect(rect.x, rect.y, rect.w, rect.h, theme::RADIUS, fill);
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, theme::BLACK);
    if matches!(state, ButtonState::Normal | ButtonState::Hover) {
        canvas.hline(rect.x + 1, rect.y + 1, rect.w - 2, lighten(fill, 22));
        canvas.vline(rect.x + 1, rect.y + 1, rect.h - 2, lighten(fill, 12));
    }
    if state == ButtonState::Pressed {
        canvas.hline(rect.x + 1, rect.y + 1, rect.w - 2, darken(fill, 22));
    }
    (fill, ink)
}

/// Small square `-` / `+` button used next to numeric inputs.
pub fn step_button(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    label: &str,
    enabled: bool,
    ui: &mut UiState,
    text_scale: i32,
) -> bool {
    if rect.w <= 0 || rect.h <= 0 {
        return false;
    }
    let hovered = enabled && ui.hovered(rect);
    let pressed = hovered && ui.mouse_down;
    let fill = if !enabled {
        theme::DISABLED
    } else if pressed {
        darken(theme::PANEL_ALT, 10)
    } else if hovered {
        theme::PANEL_HOVER
    } else {
        theme::PANEL_ALT
    };
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, fill);
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, theme::BORDER);
    let text = if enabled {
        theme::TEXT
    } else {
        theme::DISABLED_TEXT
    };
    let y = rect.y + (rect.h - Font::glyph_height(text_scale)) / 2 + i32::from(pressed);
    font.draw_centered(canvas, rect.center_x(), y, label, text, text_scale);
    enabled && ui.clicked(rect)
}

/// 12x12 checkbox with a blue cross. The caller owns the label and hit area.
pub fn checkbox(canvas: &mut Canvas, rect: Rect, checked: bool, enabled: bool, hovered: bool) {
    let box_rect = Rect::new(rect.x, rect.y, 12, 12);
    canvas.fill_rect(
        box_rect.x,
        box_rect.y,
        12,
        12,
        if enabled {
            theme::WHITE
        } else {
            theme::PANEL_ALT
        },
    );
    let border = if enabled && hovered {
        theme::BLUE
    } else if enabled {
        theme::BORDER
    } else {
        theme::DISABLED
    };
    canvas.stroke_rect(box_rect.x, box_rect.y, 12, 12, border);
    if checked {
        let color = if enabled {
            theme::BLUE
        } else {
            theme::DISABLED_TEXT
        };
        for step in 0..8 {
            canvas.fill_rect(box_rect.x + 2 + step, box_rect.y + 2 + step, 1, 1, color);
            canvas.fill_rect(box_rect.x + 9 - step, box_rect.y + 2 + step, 1, 1, color);
        }
    }
}

/// State passed to [`input`]. Editing details live in the application model.
#[derive(Clone, Copy, Debug)]
pub struct InputState<'a> {
    pub text: &'a str,
    /// Caret position in characters; `Some` means the field has focus.
    pub caret: Option<usize>,
    pub invalid: bool,
    pub enabled: bool,
    /// All text is selected and the next character replaces it.
    pub select_all: bool,
}

/// Single-line text field with a blinking caret. Returns true when clicked.
pub fn input(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    state: &InputState,
    ui: &mut UiState,
    text_scale: i32,
) -> bool {
    if rect.w <= 0 || rect.h <= 0 {
        return false;
    }
    let hovered = state.enabled && ui.hovered(rect);
    let focused = state.caret.is_some();
    canvas.fill_rect(
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        if state.enabled {
            theme::WHITE
        } else {
            theme::PANEL_ALT
        },
    );
    let border = if state.invalid && state.enabled {
        theme::RED
    } else if focused {
        theme::BLUE
    } else if hovered {
        theme::BLUE_DARK
    } else {
        theme::BORDER
    };
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, border);

    let pad = 5;
    let adv = font.advance(text_scale).max(1);
    let avail = (rect.w - pad * 2).max(adv);
    let max_chars = (avail / adv).max(1) as usize;
    let chars: Vec<char> = state.text.chars().collect();
    let caret = state.caret.unwrap_or(chars.len());
    let start = if focused {
        caret.saturating_sub(max_chars)
    } else {
        chars.len().saturating_sub(max_chars)
    };
    let end = (start + max_chars).min(chars.len());
    let visible: String = chars[start..end].iter().collect();
    let y = rect.y + (rect.h - Font::glyph_height(text_scale)) / 2;
    let color = if state.enabled {
        theme::TEXT
    } else {
        theme::DISABLED_TEXT
    };
    if state.select_all && !visible.is_empty() {
        let text_w = visible.chars().count() as i32 * adv;
        canvas.fill_rect(
            rect.x + pad,
            rect.y + 3,
            text_w.min(rect.w - pad * 2),
            rect.h - 6,
            theme::BLUE_PALE,
        );
    }
    font.draw(canvas, rect.x + pad, y, &visible, color, text_scale);
    if focused && (ui.time * 2.0) as i32 % 2 == 0 {
        let caret_col = caret.clamp(start, end);
        let cx = rect.x + pad + (caret_col - start) as i32 * adv;
        canvas.fill_rect(
            cx,
            y - 1,
            1,
            Font::glyph_height(text_scale) + 2,
            theme::TEXT,
        );
    }
    state.enabled && ui.clicked(rect)
}

/// Dropdown header: current value plus a down arrow. Returns true when clicked.
#[allow(clippy::too_many_arguments)]
pub fn dropdown_button(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    value: &str,
    open: bool,
    enabled: bool,
    ui: &mut UiState,
    text_scale: i32,
) -> bool {
    if rect.w <= 0 || rect.h <= 0 {
        return false;
    }
    let hovered = enabled && ui.hovered(rect);
    let fill = if !enabled {
        theme::PANEL_ALT
    } else if hovered {
        theme::PANEL_HOVER
    } else {
        theme::WHITE
    };
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, fill);
    let border = if open {
        theme::BLUE
    } else if hovered {
        theme::BLUE_DARK
    } else {
        theme::BORDER
    };
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, border);
    let pad = 5;
    let y = rect.y + (rect.h - Font::glyph_height(text_scale)) / 2;
    let color = if enabled {
        theme::TEXT
    } else {
        theme::DISABLED_TEXT
    };
    let arrow = "↓";
    let arrow_w = font.width(arrow, text_scale);
    font.draw(canvas, rect.x + pad, y, value, color, text_scale);
    font.draw(
        canvas,
        rect.right() - pad - arrow_w,
        y,
        arrow,
        theme::MUTED,
        text_scale,
    );
    enabled && ui.clicked(rect)
}

/// Mode tab in the toolbar.
///
/// The active tab is white with a blue bottom rule, the others sit on the
/// toolbar colour. Clicking the active tab does nothing. Returns true when an
/// inactive tab was clicked.
#[allow(clippy::too_many_arguments)]
pub fn tab(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    label: &str,
    active: bool,
    enabled: bool,
    ui: &mut UiState,
    text_scale: i32,
) -> bool {
    if rect.w <= 0 || rect.h <= 0 {
        return false;
    }
    let hovered = enabled && !active && ui.hovered(rect);
    let fill = if !enabled {
        theme::PANEL_ALT
    } else if active {
        theme::WHITE
    } else if hovered {
        theme::PANEL_HOVER
    } else {
        theme::PANEL_ALT
    };
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, fill);
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, theme::BORDER);
    if active {
        canvas.hline(rect.x + 1, rect.bottom() - 1, rect.w - 2, theme::BLUE);
        canvas.hline(rect.x + 1, rect.bottom() - 2, rect.w - 2, theme::BLUE);
        canvas.hline(rect.x + 1, rect.bottom() - 3, rect.w - 2, theme::BLUE_DARK);
    }
    let color = if !enabled {
        theme::DISABLED_TEXT
    } else if active {
        theme::BLUE_DARK
    } else {
        theme::TEXT
    };
    let scale = fit_scale(font, label, rect.w - 16, text_scale);
    let y = rect.y + (rect.h - Font::glyph_height(scale)) / 2;
    font.draw_centered(canvas, rect.center_x(), y, label, color, scale);
    enabled && !active && ui.clicked(rect)
}

/// Popup background for a dropdown list.
pub fn dropdown_panel(canvas: &mut Canvas, rect: Rect) {
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    canvas.fill_rect(
        rect.x + theme::SHADOW,
        rect.y + theme::SHADOW,
        rect.w,
        rect.h,
        theme::BLACK,
    );
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, theme::WHITE);
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, theme::BLACK);
}

/// One row of a dropdown list. Returns true when clicked.
pub fn menu_item(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    label: &str,
    selected: bool,
    ui: &mut UiState,
    text_scale: i32,
) -> bool {
    if rect.w <= 0 || rect.h <= 0 {
        return false;
    }
    let hovered = ui.hovered(rect);
    let fill = if selected {
        theme::BLUE
    } else if hovered {
        theme::BLUE_PALE
    } else {
        theme::WHITE
    };
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, fill);
    let color = if selected { theme::WHITE } else { theme::TEXT };
    let y = rect.y + (rect.h - Font::glyph_height(text_scale)) / 2;
    font.draw(canvas, rect.x + 8, y, label, color, text_scale);
    ui.clicked(rect)
}

/// Vertical scrollbar, drawn only when the content overflows.
pub fn scrollbar(
    canvas: &mut Canvas,
    rect: Rect,
    offset: i32,
    max_offset: i32,
    content_h: i32,
    view_h: i32,
) {
    if rect.w <= 0 || rect.h <= 0 || max_offset <= 0 || content_h <= 0 || view_h <= 0 {
        return;
    }
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, theme::PANEL_ALT);
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, theme::BORDER);
    let track = rect.h - 2;
    let thumb_h = (((view_h as f32 / content_h as f32) * track as f32).round() as i32)
        .clamp(18, track.max(18));
    let travel = (track - thumb_h).max(1);
    let ratio = (offset.clamp(0, max_offset) as f32 / max_offset as f32).clamp(0.0, 1.0);
    let thumb_y = rect.y + 1 + (ratio * travel as f32).round() as i32;
    canvas.fill_rect(rect.x + 1, thumb_y, rect.w - 2, thumb_h, theme::MUTED);
}

/// Section caption with a thin rule underneath.
pub fn section_title(
    canvas: &mut Canvas,
    font: &Font,
    x: i32,
    y: i32,
    width: i32,
    title: &str,
    scale: i32,
) {
    font.draw(canvas, x, y, title, theme::TEXT, scale);
    let rule_y = y + Font::glyph_height(scale) + 4;
    canvas.hline(x, rule_y, width, theme::BORDER);
}

/// Bottom status strip.
pub fn status_bar(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    left: &str,
    right: &str,
    scale: i32,
) {
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, theme::PANEL_ALT);
    canvas.hline(rect.x, rect.y, rect.w, theme::BORDER);
    let y = rect.y + (rect.h - Font::glyph_height(scale)) / 2;
    font.draw(canvas, rect.x + 12, y, left, theme::TEXT, scale);
    font.draw_right(canvas, rect.right() - 12, y, right, theme::MUTED, scale);
}

/// Dim the whole canvas with a static dither scrim.
///
/// The pattern must not depend on time: animating the phase made the whole
/// background flash on every redraw (e.g. while the pointer moved over the
/// "СОХРАНЕНО" dialog).
pub fn scrim(canvas: &mut Canvas) {
    let width = canvas.width();
    let height = canvas.height();
    canvas.dither_rect(0, 0, width, height, theme::BLACK, 0);
}

/// Header tone of a modal dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogTone {
    Error,
    Info,
    Warning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogButtons {
    Ok,
    OkCancel,
}

/// Text content and buttons of a modal dialog.
pub struct DialogSpec<'a> {
    pub title: &'a str,
    pub lines: &'a [String],
    pub tone: DialogTone,
    pub buttons: DialogButtons,
    pub ok_label: &'a str,
    pub cancel_label: &'a str,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DialogResponse {
    pub ok: bool,
    pub cancel: bool,
}

/// Centered modal dialog. Returns which button was clicked.
pub fn dialog(
    canvas: &mut Canvas,
    font: &Font,
    viewport: Rect,
    spec: &DialogSpec,
    ui: &mut UiState,
    metrics: Metrics,
) -> DialogResponse {
    let line_h = Font::line_height(metrics.body) + 2;
    let body_h = spec.lines.len().max(1) as i32 * line_h;
    let width = (viewport.w - 48).clamp(320, 560);
    let height = (theme::HEADER_H + 20 + body_h + 16 + theme::BUTTON_H + 20)
        .min(viewport.h - 32)
        .max(180);
    let panel = dialog_frame(
        canvas,
        font,
        viewport,
        spec.title,
        width,
        height,
        spec.tone,
        metrics,
    );

    let mut y = panel.y + theme::HEADER_H + 16;
    for line in spec.lines {
        font.draw(canvas, panel.x + 18, y, line, theme::TEXT, metrics.body);
        y += line_h;
    }

    let ok_w = 110;
    let cancel_w = 110;
    let mut response = DialogResponse::default();
    let ok_x = panel.right() - ok_w - 18;
    let ok = Rect::new(
        ok_x,
        panel.bottom() - theme::BUTTON_H - 14,
        ok_w,
        theme::BUTTON_H,
    );
    if spec.buttons == DialogButtons::OkCancel {
        let cancel = Rect::new(
            ok_x - cancel_w - 10,
            panel.bottom() - theme::BUTTON_H - 14,
            cancel_w,
            theme::BUTTON_H,
        );
        if button_tone(
            canvas,
            font,
            cancel,
            spec.cancel_label,
            true,
            ui,
            metrics.body,
            ButtonTone::Secondary,
        ) {
            response.cancel = true;
        }
    }
    if button_tone(
        canvas,
        font,
        ok,
        spec.ok_label,
        true,
        ui,
        metrics.body,
        ButtonTone::Primary,
    ) {
        response.ok = true;
    }
    response
}

/// Dither scrim, hard shadow, white body and a coloured title bar of a modal
/// dialog, centred in `viewport`. Returns the panel rect; the caller lays out
/// the body below `panel.y + HEADER_H`.
#[allow(clippy::too_many_arguments)]
pub fn dialog_frame(
    canvas: &mut Canvas,
    font: &Font,
    viewport: Rect,
    title: &str,
    width: i32,
    height: i32,
    tone: DialogTone,
    metrics: Metrics,
) -> Rect {
    scrim(canvas);

    let panel = Rect::new(
        viewport.x + (viewport.w - width) / 2,
        viewport.y + (viewport.h - height) / 2,
        width,
        height,
    );

    canvas.fill_rect(
        panel.x + theme::SHADOW,
        panel.y + theme::SHADOW,
        panel.w,
        panel.h,
        theme::BLACK,
    );
    canvas.fill_rect(panel.x, panel.y, panel.w, panel.h, theme::WHITE);
    canvas.stroke_rect(panel.x, panel.y, panel.w, panel.h, theme::BLACK);
    let header = Rect::new(panel.x, panel.y, panel.w, theme::HEADER_H);
    let header_color = match tone {
        DialogTone::Error => theme::RED,
        DialogTone::Info | DialogTone::Warning => theme::BLUE,
    };
    canvas.fill_rect(header.x, header.y, header.w, header.h, header_color);
    canvas.hline(header.x, header.y, header.w, lighten(header_color, 25));
    canvas.hline(
        header.x,
        header.bottom() - 1,
        header.w,
        darken(header_color, 25),
    );
    let title_y = header.y + (header.h - Font::glyph_height(metrics.body)) / 2;
    font.draw(
        canvas,
        header.x + 14,
        title_y,
        title,
        theme::WHITE,
        metrics.body,
    );
    panel
}

/// Legacy single-button error modal. Returns true when "OK" is clicked.
pub fn modal(
    canvas: &mut Canvas,
    font: &Font,
    viewport: Rect,
    title: &str,
    lines: &[String],
    ui: &mut UiState,
    metrics: Metrics,
) -> bool {
    let spec = DialogSpec {
        title,
        lines,
        tone: DialogTone::Error,
        buttons: DialogButtons::Ok,
        ok_label: "ОК",
        cancel_label: "",
    };
    dialog(canvas, font, viewport, &spec, ui, metrics).ok
}

fn fit_scale(font: &Font, text: &str, max_width: i32, preferred: i32) -> i32 {
    let mut scale = preferred.max(1);
    while scale > 1 && font.width(text, scale) > max_width {
        scale -= 1;
    }
    scale
}

/// Small toolbar chip (zoom controls). Returns true when clicked.
///
/// Unlike a button the chip has no shadow: a row of chips stays compact inside
/// the preview header. `active` marks the currently selected zoom mode.
#[allow(clippy::too_many_arguments)]
pub fn chip(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    label: &str,
    active: bool,
    enabled: bool,
    ui: &mut UiState,
    text_scale: i32,
) -> bool {
    if rect.w <= 0 || rect.h <= 0 {
        return false;
    }
    let hovered = enabled && ui.hovered(rect);
    let fill = if !enabled {
        theme::DISABLED
    } else if active {
        theme::BLUE
    } else if hovered {
        theme::PANEL_HOVER
    } else {
        theme::WHITE
    };
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, fill);
    canvas.stroke_rect(
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        if active {
            theme::BLUE_DARK
        } else {
            theme::BORDER
        },
    );
    let color = if !enabled {
        theme::DISABLED_TEXT
    } else if active {
        theme::WHITE
    } else {
        theme::TEXT
    };
    let scale = fit_scale(font, label, rect.w - 10, text_scale);
    let y = rect.y + (rect.h - Font::glyph_height(scale)) / 2;
    font.draw_centered(canvas, rect.center_x(), y, label, color, scale);
    enabled && ui.clicked(rect)
}

/// Static chip that only displays a value (current zoom percentage).
pub fn badge(canvas: &mut Canvas, font: &Font, rect: Rect, label: &str, text_scale: i32) {
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, theme::PANEL_ALT);
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, theme::BORDER);
    let scale = fit_scale(font, label, rect.w - 10, text_scale);
    let y = rect.y + (rect.h - Font::glyph_height(scale)) / 2;
    font.draw_centered(canvas, rect.center_x(), y, label, theme::TEXT, scale);
}

/// 1px accent ring around a control that owns keyboard focus.
pub fn focus_ring(canvas: &mut Canvas, rect: Rect) {
    if rect.w <= 0 || rect.h <= 0 {
        return;
    }
    canvas.stroke_rect(rect.x - 2, rect.y - 2, rect.w + 4, rect.h + 4, theme::BLUE);
}

/// Hover tooltip card placed below (or above) an anchor control.
///
/// The text is wrapped to a readable width and the card is clamped to the
/// canvas, so tooltips of controls on the right edge stay visible.
pub fn tooltip(canvas: &mut Canvas, font: &Font, anchor: Rect, text: &str, metrics: Metrics) {
    if text.is_empty() || canvas.width() < 120 {
        return;
    }
    let max_text_w = 360.min(canvas.width() - 40).max(100);
    let lines = wrap_text(font, text, max_text_w, metrics.small);
    if lines.is_empty() {
        return;
    }
    let line_h = Font::line_height(metrics.small);
    let text_w = lines
        .iter()
        .map(|line| font.width(line, metrics.small))
        .max()
        .unwrap_or(0);
    let w = text_w + 16;
    let h = lines.len() as i32 * line_h + 10;
    let x = (anchor.center_x() - w / 2).clamp(8, (canvas.width() - w - 8).max(8));
    let mut y = anchor.bottom() + 6;
    if y + h > canvas.height() - 8 {
        y = (anchor.y - h - 6).max(8);
    }
    canvas.fill_rect(x + 2, y + 2, w, h, theme::BLACK);
    canvas.fill_rect(x, y, w, h, theme::WHITE);
    canvas.stroke_rect(x, y, w, h, theme::BLUE_DARK);
    let mut text_y = y + 5;
    for line in &lines {
        font.draw(canvas, x + 8, text_y, line, theme::TEXT, metrics.small);
        text_y += line_h;
    }
}

/// Colour family of a toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastTone {
    Info,
    Error,
}

/// Bottom-center toast card. Layout and lifetime are owned by the caller.
pub fn toast_card(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    lines: &[String],
    tone: ToastTone,
    metrics: Metrics,
) {
    if rect.w <= 0 || rect.h <= 0 || lines.is_empty() {
        return;
    }
    let fill = match tone {
        ToastTone::Info => theme::BLUE,
        ToastTone::Error => theme::RED,
    };
    canvas.fill_rect(
        rect.x + theme::SHADOW,
        rect.y + theme::SHADOW,
        rect.w,
        rect.h,
        theme::BLACK,
    );
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, fill);
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, theme::BLACK);
    canvas.hline(rect.x, rect.y, rect.w, lighten(fill, 25));
    let line_h = Font::line_height(metrics.small);
    let mut y = rect.y + 5;
    for line in lines {
        font.draw(canvas, rect.x + 10, y, line, theme::WHITE, metrics.small);
        y += line_h;
    }
}

/// Greedy character wrap used by tooltips. Uppercase bitmap text has a fixed
/// advance, so a plain character count is enough.
fn wrap_text(font: &Font, text: &str, max_width: i32, scale: i32) -> Vec<String> {
    let adv = font.advance(scale).max(1);
    let max_chars = (max_width / adv).max(4) as usize;
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let extra = usize::from(!line.is_empty());
            if !line.is_empty() && line.chars().count() + extra + word.chars().count() > max_chars {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            if word.chars().count() > max_chars {
                // A long unbroken token (a path) is split by characters.
                for ch in word.chars() {
                    if line.chars().count() >= max_chars {
                        lines.push(std::mem::take(&mut line));
                    }
                    line.push(ch);
                }
            } else {
                line.push_str(word);
            }
        }
        lines.push(line);
    }
    lines
}

/// Small helper so callers do not need to import `Color` just to tint text.
pub fn text_color(color: Color) -> Color {
    color
}
