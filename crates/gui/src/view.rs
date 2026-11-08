//! Drawing and hit-testing for the editor window.
//!
//! The view never mutates application state. It returns an [`Action`] for the
//! event handler to apply after the frame has been presented.

use pixel_image_core::Color as CoreColor;
use pixel_image_ui::{
    Canvas, Color, Font, Metrics, Point, Rect, UiState,
    color::over,
    icons,
    image::fit_rect,
    theme::{
        self, BUTTON_H, GAP, HEADER_H, LEFT_PANEL_W, OUTER_MARGIN, PREVIEW_HEADER_H, STATUS_H,
        TOOLBAR_H,
    },
    widgets::{
        self, ButtonTone, DialogButtons, DialogSpec, DialogTone, InputState,
        ToastTone as UiToastTone,
    },
};

use crate::model::{
    CutoutImage, Field, FocusId, GeneratedImage, ModalButtons, ModalTone, Mode, Model, PixelParams,
    SaveTarget, Status, ToastTone, Toggle, ZoomMode, filter_hint, filter_label, filter_values,
};

/// Height of a mode tab in the toolbar.
const TAB_H: i32 = 30;
/// Width of the icon-only save/export buttons at the right edge of the toolbar.
const ICON_BUTTON_W: i32 = 36;
/// Height of the compact zoom chips in the preview header.
const CHIP_H: i32 = 20;
/// How many recent files the empty state lists.
const RECENT_VISIBLE: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Open,
    OpenRecent(usize),
    Clear,
    SetMode(Mode),
    Generate,
    Save,
    /// Toolbar export icon: opens the export dialog.
    Export,
    /// "СОХРАНИТЬ" in the export dialog: starts the export.
    ConfirmExport,
    /// "ОТМЕНА" in the export dialog: drops the folder/prefix changes.
    CancelExport,
    Undo,
    Redo,
    /// Settings header icon: restores the panel to the default values.
    ResetSettings,
    PickOutputDir,
    PickCornerColor,
    ConfirmModal,
    CancelModal,
    BeginEdit(Field),
    Step(Field, i32),
    Toggle(Toggle),
    ToggleDropdown,
    ChooseFilter(pixel_image_core::Filter),
    SetScroll(i32),
    ZoomIn,
    ZoomOut,
    ZoomFit,
}

/// Result of one view pass: the action plus layout facts the event loop needs.
pub struct Frame {
    pub action: Action,
    pub scroll_max: i32,
    /// Left panel rect, used to route the mouse wheel to the settings list.
    pub sidebar: Rect,
    /// Preview body rect, used for wheel zoom and pan dragging.
    pub preview: Rect,
    /// Largest pan offset for the current zoom, per axis.
    pub pan_limit: Point,
    /// Measured "по размеру" percentage of the primary preview image.
    pub fit_percent: Option<u32>,
}

impl Frame {
    fn empty() -> Self {
        Self {
            action: Action::None,
            scroll_max: 0,
            sidebar: Rect::default(),
            preview: Rect::default(),
            pan_limit: Point::new(0, 0),
            fit_percent: None,
        }
    }
}

/// Hover tooltip candidate: the anchor control and its text.
type Tip = Option<(Rect, String)>;

pub fn draw(
    canvas: &mut Canvas,
    model: &Model,
    ui: &mut UiState,
    font: &Font,
    metrics: Metrics,
) -> Frame {
    canvas.clear(theme::DESKTOP);

    let width = canvas.width();
    let height = canvas.height();
    let outer = Rect::new(
        OUTER_MARGIN,
        OUTER_MARGIN,
        width - OUTER_MARGIN * 2,
        height - OUTER_MARGIN * 2,
    );
    if outer.w < 640 || outer.h < 380 {
        return Frame::empty();
    }

    widgets::panel(canvas, outer);
    let header = Rect::new(outer.x + 1, outer.y + 1, outer.w - 2, HEADER_H);
    widgets::window_header(
        canvas,
        font,
        header,
        "PIXEL-IMAGE",
        concat!("ВЕРСИЯ ", env!("CARGO_PKG_VERSION")),
        metrics,
    );

    let toolbar = Rect::new(outer.x + 1, header.bottom(), outer.w - 2, TOOLBAR_H);
    canvas.fill_rect(toolbar.x, toolbar.y, toolbar.w, toolbar.h, theme::PANEL_ALT);
    canvas.hline(toolbar.x, toolbar.bottom() - 1, toolbar.w, theme::BORDER);

    let status = Rect::new(
        outer.x + 1,
        outer.bottom() - 1 - STATUS_H,
        outer.w - 2,
        STATUS_H,
    );
    let content = Rect::new(
        outer.x + 1,
        toolbar.bottom(),
        outer.w - 2,
        status.y - toolbar.bottom(),
    );

    // A modal, the export dialog or an open dropdown must block the controls
    // underneath. The click is restored before the overlay is drawn so the
    // overlay can consume it.
    let blocked = model.modal.is_some() || model.export_dialog_open() || model.dropdown_open;
    let saved_click = ui.click_pending;
    if blocked {
        ui.click_pending = false;
    }

    let mut tip: Tip = None;
    let mut action = Action::None;
    action = merge(
        action,
        draw_toolbar(canvas, model, ui, font, metrics, toolbar, &mut tip),
    );

    let left = Rect::new(content.x + 12, content.y + 12, LEFT_PANEL_W, content.h - 24);
    let preview = Rect::new(
        left.right() + GAP,
        content.y + 12,
        content.right() - left.right() - GAP - 12,
        content.h - 24,
    );
    let (sidebar_action, scroll_max, dropdown_anchor) =
        draw_sidebar(canvas, model, ui, font, metrics, left, &mut tip);
    action = merge(action, sidebar_action);
    let preview_info = draw_preview(canvas, model, ui, font, metrics, preview, &mut tip);
    action = merge(action, preview_info.action);
    draw_status_bar(canvas, model, font, metrics, status);

    // Native drag&drop hint: the OS reports the hovered file before the drop.
    if let Some(path) = &model.drop_hover {
        draw_drop_overlay(canvas, font, metrics, preview_info.body, path);
    }

    if blocked {
        ui.click_pending = saved_click;
    }

    if model.dropdown_open
        && let Some(anchor) = dropdown_anchor
    {
        action = merge(
            action,
            draw_filter_menu(canvas, model, ui, font, metrics, anchor, left),
        );
    }

    if model.modal.is_none()
        && !model.export_dialog_open()
        && !model.dropdown_open
        && let Some((anchor, text)) = tip
    {
        widgets::tooltip(canvas, font, anchor, &text, metrics);
    }

    draw_toasts(canvas, model, font, metrics, status.y - 10);

    if let Some(modal) = &model.modal {
        let wrap_width = (width.min(560) - 40).max(240);
        let lines = wrap_limit(font, &modal.message, wrap_width, metrics.body, 12);
        let spec = DialogSpec {
            title: &modal.title,
            lines: &lines,
            tone: match modal.tone {
                ModalTone::Error => DialogTone::Error,
                ModalTone::Info => DialogTone::Info,
                ModalTone::Warning => DialogTone::Warning,
            },
            buttons: match modal.buttons {
                ModalButtons::Ok => DialogButtons::Ok,
                ModalButtons::OkCancel => DialogButtons::OkCancel,
            },
            ok_label: "ОК",
            cancel_label: "ОТМЕНА",
        };
        let response = widgets::dialog(
            canvas,
            font,
            Rect::new(0, 0, width, height),
            &spec,
            ui,
            metrics,
        );
        if response.ok {
            action = Action::ConfirmModal;
        } else if response.cancel {
            action = Action::CancelModal;
        }
    } else if model.export_dialog_open() {
        action = merge(
            action,
            draw_export_dialog(canvas, model, ui, font, metrics, Rect::new(0, 0, width, height)),
        );
    }

    Frame {
        action,
        scroll_max,
        sidebar: left,
        preview: preview_info.body,
        pan_limit: preview_info.pan_limit,
        fit_percent: preview_info.fit_percent,
    }
}

fn merge(current: Action, next: Action) -> Action {
    if next == Action::None { current } else { next }
}

/// Remember the first hovered control as the tooltip of the frame.
fn hover_tip(ui: &UiState, tip: &mut Tip, rect: Rect, text: impl Into<String>) {
    if tip.is_none() && !rect.is_empty() && ui.hovered(rect) {
        *tip = Some((rect, text.into()));
    }
}

fn focused(model: &Model, id: FocusId) -> bool {
    model.focus == Some(id)
}

#[allow(clippy::too_many_arguments)]
fn draw_toolbar(
    canvas: &mut Canvas,
    model: &Model,
    ui: &mut UiState,
    font: &Font,
    metrics: Metrics,
    toolbar: Rect,
    tip: &mut Tip,
) -> Action {
    let button_y = toolbar.y + (toolbar.h - BUTTON_H) / 2;
    let mut x = toolbar.x + 12;
    let mut action = Action::None;

    // Mode tabs choose the pipeline, the settings section and the save target.
    let tab_y = toolbar.y + (toolbar.h - TAB_H) / 2;
    for mode in Mode::ALL {
        let tab_w = (font.width(mode.label(), metrics.body) + 30).max(140);
        let rect = Rect::new(x, tab_y, tab_w, TAB_H);
        if widgets::tab(
            canvas,
            font,
            rect,
            mode.label(),
            model.mode == mode,
            true,
            ui,
            metrics.body,
        ) {
            action = Action::SetMode(mode);
        }
        if focused(model, FocusId::ModeTab(mode)) {
            widgets::focus_ring(canvas, rect);
        }
        hover_tip(
            ui,
            tip,
            rect,
            match mode {
                Mode::PixelArt => "ПИКСЕЛЬ-АРТ: ШИРИНА, ПАЛИТРА, СЕТКА; CTRL+G — СГЕНЕРИРОВАТЬ",
                Mode::Cutout => "ВЫРЕЗКА: УДАЛЕНИЕ ФОНА В ИСХОДНОМ РАЗМЕРЕ; CTRL+G — ВЫРЕЗАТЬ",
            },
        );
        x = rect.right();
    }
    x += 14;
    canvas.vline(x - 7, toolbar.y + 9, toolbar.h - 18, theme::BORDER);

    let open_label = if model.file.is_some() {
        "ЗАМЕНИТЬ"
    } else {
        "ОТКРЫТЬ ФАЙЛ"
    };
    let open_w = (font.width(open_label, metrics.body) + 22).max(144);
    let open = Rect::new(x, button_y, open_w, BUTTON_H);
    if widgets::button(canvas, font, open, open_label, true, ui, metrics.body) {
        action = Action::Open;
    }
    if focused(model, FocusId::Open) {
        widgets::focus_ring(canvas, open);
    }
    hover_tip(ui, tip, open, "ОТКРЫТЬ ИЛИ ЗАМЕНИТЬ ИЗОБРАЖЕНИЕ (CTRL+O)");
    // Tight gap: at the 1120 px minimum the buttons still fit on one row.
    const BUTTON_GAP: i32 = 6;
    x = open.right() + BUTTON_GAP;

    let clear_enabled = model.file.is_some() || model.is_busy();
    let clear_label = "ОЧИСТИТЬ";
    let clear_w = (font.width(clear_label, metrics.body) + 22).max(130);
    let clear = Rect::new(x, button_y, clear_w, BUTTON_H);
    if widgets::button(
        canvas,
        font,
        clear,
        clear_label,
        clear_enabled,
        ui,
        metrics.body,
    ) {
        action = Action::Clear;
    }
    if focused(model, FocusId::Clear) {
        widgets::focus_ring(canvas, clear);
    }
    hover_tip(ui, tip, clear, "УБРАТЬ ФАЙЛ ИЗ ОКНА (CTRL+L)");
    x = clear.right() + BUTTON_GAP;

    let run_label = match model.mode {
        Mode::PixelArt => "СГЕНЕРИРОВАТЬ",
        Mode::Cutout => "ВЫРЕЗАТЬ",
    };
    let run_w = (font.width(run_label, metrics.body) + 22).max(150);
    let run = Rect::new(x, button_y, run_w, BUTTON_H);
    if widgets::button_tone(
        canvas,
        font,
        run,
        run_label,
        model.can_generate(),
        ui,
        metrics.body,
        ButtonTone::Danger,
    ) {
        action = Action::Generate;
    }
    if focused(model, FocusId::Generate) {
        widgets::focus_ring(canvas, run);
    }
    hover_tip(
        ui,
        tip,
        run,
        "ПОСТРОИТЬ РЕЗУЛЬТАТ ПО ТЕКУЩИМ НАСТРОЙКАМ (CTRL+G)",
    );
    x = run.right();

    // Save and export are icon-only buttons pinned to the right edge of the
    // toolbar; their tooltips carry the labels and shortcuts. Both slots stay
    // reserved when export is hidden, so the row does not shift on open/clear.
    let right = toolbar.right() - 12;
    let export = Rect::new(right - ICON_BUTTON_W, button_y, ICON_BUTTON_W, BUTTON_H);
    let save = Rect::new(
        export.x - BUTTON_GAP - ICON_BUTTON_W,
        button_y,
        ICON_BUTTON_W,
        BUTTON_H,
    );

    if widgets::icon_button(
        canvas,
        save,
        &icons::SAVE,
        model.can_save(),
        ui,
        ButtonTone::Primary,
    ) {
        action = Action::Save;
    }
    if focused(model, FocusId::Save) {
        widgets::focus_ring(canvas, save);
    }
    hover_tip(ui, tip, save, "СОХРАНИТЬ РЯДОМ С ИСХОДНИКОМ (CTRL+S)");

    // Export opens the dialog with the output folder and prefix. It is hidden
    // with no file, which also keeps the empty toolbar inside the minimum
    // window width.
    if model.file.is_some() {
        if widgets::icon_button(
            canvas,
            export,
            &icons::EXPORT,
            model.can_save(),
            ui,
            ButtonTone::Secondary,
        ) {
            action = Action::Export;
        }
        if focused(model, FocusId::Export) {
            widgets::focus_ring(canvas, export);
        }
        hover_tip(
            ui,
            tip,
            export,
            "ОКНО ЭКСПОРТА: ПАПКА И ПРЕФИКС (CTRL+E)",
        );
    }

    if model.file.is_none() {
        let hint = "PNG / JPG / WEBP / BMP / GIF";
        let hint_w = font.width(hint, metrics.small);
        // Right-aligned against the icon group so the two never overlap.
        let hint_right = save.x - 12;
        // At the minimum window width the tabs and buttons leave no room for
        // the format hint; the empty state already repeats it.
        if hint_right - hint_w > x + 12 {
            let y = toolbar.y + (toolbar.h - Font::glyph_height(metrics.small)) / 2;
            font.draw_right(canvas, hint_right, y, hint, theme::MUTED, metrics.small);
        }
    }

    action
}

/// Modal "ЭКСПОРТ" dialog: output folder with "ОБЗОР", the prefix field, the
/// file names it is going to write and "СОХРАНИТЬ" / "ОТМЕНА".
fn draw_export_dialog(
    canvas: &mut Canvas,
    model: &Model,
    ui: &mut UiState,
    font: &Font,
    metrics: Metrics,
    viewport: Rect,
) -> Action {
    let width = (viewport.w - 48).clamp(360, 560);
    let height = 240.min(viewport.h - 32).max(200);
    let panel = widgets::dialog_frame(
        canvas,
        font,
        viewport,
        "ЭКСПОРТ",
        width,
        height,
        DialogTone::Info,
        metrics,
    );

    let pad = 20;
    let left = panel.x + pad;
    let inner_w = panel.w - pad * 2;
    let body_top = panel.y + theme::HEADER_H + 14;
    let mut action = Action::None;

    // Folder: label, native picker and the resolved path underneath.
    font.draw(canvas, left, body_top, "ПАПКА", theme::TEXT, metrics.body);
    let browse = Rect::new(panel.right() - pad - 86, body_top - 5, 86, 24);
    if widgets::button_tone(
        canvas,
        font,
        browse,
        "ОБЗОР",
        true,
        ui,
        metrics.body,
        ButtonTone::Secondary,
    ) {
        action = Action::PickOutputDir;
    }
    if focused(model, FocusId::PickOutputDir) {
        widgets::focus_ring(canvas, browse);
    }
    let path = elide_middle(font, &model.output_dir_display(), inner_w, metrics.small);
    font.draw(canvas, left, body_top + 20, &path, theme::MUTED, metrics.small);

    // Prefix: the same text field the settings panel uses.
    let prefix_y = body_top + 42;
    font.draw(canvas, left, prefix_y, "ПРЕФИКС", theme::TEXT, metrics.body);
    let field = Rect::new(left, prefix_y + 18, inner_w, theme::INPUT_H + 2);
    let text = model.display_text(Field::OutputPrefix);
    let state = InputState {
        text: &text,
        caret: model.display_caret(Field::OutputPrefix),
        invalid: model.display_invalid(Field::OutputPrefix),
        enabled: true,
        select_all: model.display_select_all(Field::OutputPrefix),
    };
    if widgets::input(canvas, font, field, &state, ui, metrics.body) {
        action = Action::BeginEdit(Field::OutputPrefix);
    }
    if focused(model, FocusId::Field(Field::OutputPrefix)) {
        widgets::focus_ring(canvas, field);
    }

    // Resulting file names, then the reason the export cannot start.
    let names = model.export_preview_names();
    if !names.is_empty() {
        let text = format!("ФАЙЛЫ: {}", names.join(" + "));
        let text = elide_middle(font, &text, inner_w, metrics.small);
        font.draw(
            canvas,
            left,
            field.bottom() + 8,
            &text,
            theme::MUTED,
            metrics.small,
        );
    }
    if let Some(message) = model.export_dialog_error() {
        let mut y = field.bottom() + 22;
        for line in wrap_limit(font, &message, inner_w, metrics.small, 2) {
            font.draw(canvas, left, y, &line, theme::RED, metrics.small);
            y += Font::line_height(metrics.small);
        }
    }

    // "СОХРАНИТЬ" starts the export, "ОТМЕНА" restores the previous output.
    let save_w = (font.width("СОХРАНИТЬ", metrics.body) + 30).max(120);
    let cancel_w = 110;
    let save = Rect::new(
        panel.right() - pad - save_w,
        panel.bottom() - theme::BUTTON_H - 14,
        save_w,
        theme::BUTTON_H,
    );
    let cancel = Rect::new(save.x - cancel_w - 10, save.y, cancel_w, theme::BUTTON_H);
    if widgets::button_tone(
        canvas,
        font,
        cancel,
        "ОТМЕНА",
        true,
        ui,
        metrics.body,
        ButtonTone::Secondary,
    ) {
        action = Action::CancelExport;
    }
    if focused(model, FocusId::ExportCancel) {
        widgets::focus_ring(canvas, cancel);
    }
    if widgets::button_tone(
        canvas,
        font,
        save,
        "СОХРАНИТЬ",
        true,
        ui,
        metrics.body,
        ButtonTone::Primary,
    ) {
        action = Action::ConfirmExport;
    }
    if focused(model, FocusId::ExportSave) {
        widgets::focus_ring(canvas, save);
    }

    action
}

fn draw_sidebar(
    canvas: &mut Canvas,
    model: &Model,
    ui: &mut UiState,
    font: &Font,
    metrics: Metrics,
    rect: Rect,
    tip: &mut Tip,
) -> (Action, i32, Option<Rect>) {
    if rect.is_empty() {
        return (Action::None, 0, None);
    }
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, theme::PANEL);
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, theme::BORDER);

    let pad = theme::SIDEBAR_PAD;
    let x = rect.x + pad;
    let inner_w = rect.w - pad * 2;
    let mut y = rect.y + 12;
    widgets::section_title(canvas, font, x, y, inner_w, "НАСТРОЙКИ", metrics.body);

    // The header row stays above the scrolling list: "СБРОСИТЬ" follows the
    // caption, undo/redo are pinned to the right edge so they are always
    // reachable. All three are icon-only chips with tooltips and shortcuts.
    let mut action = Action::None;
    let history_h = 20;
    let history_y = rect.y + 12 + (Font::glyph_height(metrics.body) - history_h) / 2;
    let caption_right = x + font.width("НАСТРОЙКИ", metrics.body);
    let reset_rect = Rect::new(caption_right + 10, history_y, history_h, history_h);
    if widgets::icon_chip(canvas, reset_rect, &icons::RESET, true, ui) {
        action = Action::ResetSettings;
    }
    if focused(model, FocusId::ResetSettings) {
        widgets::focus_ring(canvas, reset_rect);
    }
    hover_tip(
        ui,
        tip,
        reset_rect,
        "СБРОСИТЬ НАСТРОЙКИ К ЗНАЧЕНИЯМ ПО УМОЛЧАНИЮ",
    );

    let redo_rect = Rect::new(
        rect.right() - pad - history_h,
        history_y,
        history_h,
        history_h,
    );
    let undo_rect = Rect::new(redo_rect.x - 4 - history_h, history_y, history_h, history_h);
    if widgets::icon_chip(canvas, undo_rect, &icons::UNDO, model.can_undo(), ui) {
        action = Action::Undo;
    }
    if focused(model, FocusId::Undo) {
        widgets::focus_ring(canvas, undo_rect);
    }
    hover_tip(ui, tip, undo_rect, "ОТМЕНИТЬ ИЗМЕНЕНИЕ (CTRL+Z)");
    if widgets::icon_chip(canvas, redo_rect, &icons::REDO, model.can_redo(), ui) {
        action = Action::Redo;
    }
    if focused(model, FocusId::Redo) {
        widgets::focus_ring(canvas, redo_rect);
    }
    hover_tip(ui, tip, redo_rect, "ПОВТОРИТЬ ИЗМЕНЕНИЕ (CTRL+SHIFT+Z)");

    y += Font::glyph_height(metrics.body) + 10;

    let source_h = 140;
    let support_h = 28;
    let pinned_top = rect.bottom() - 12 - support_h - source_h;
    let viewport = Rect::new(
        x,
        y,
        inner_w - theme::SCROLL_W - 6,
        (pinned_top - 8 - y).max(0),
    );

    let rows = settings_rows(model);
    let total: i32 = rows.iter().map(row_height).sum();
    let max_scroll = (total - viewport.h).max(0);
    let offset = model.scroll.clamp(0, max_scroll);
    let mut anchor = None;

    if viewport.h > 0 {
        canvas.set_clip(viewport);
        let (sidebar_action, sidebar_anchor) = {
            let mut sidebar = Sidebar {
                canvas,
                model,
                ui,
                font,
                metrics,
                action: Action::None,
                anchor: None,
                tip,
                viewport,
            };
            sidebar.draw_rows(x, viewport.y + 2, viewport.w, offset);
            (sidebar.action, sidebar.anchor)
        };
        canvas.clear_clip();
        action = merge(action, sidebar_action);
        anchor = sidebar_anchor;
        let scroll_rect = Rect::new(
            viewport.right() + 6,
            viewport.y,
            theme::SCROLL_W,
            viewport.h,
        );
        widgets::scrollbar(canvas, scroll_rect, offset, max_scroll, total, viewport.h);
        let scroll_clicked = max_scroll > 0 && ui.clicked(scroll_rect);
        if scroll_clicked && let Some(pointer) = ui.pointer {
            let ratio =
                ((pointer.y - scroll_rect.y) as f32 / scroll_rect.h.max(1) as f32).clamp(0.0, 1.0);
            action = merge(
                action,
                Action::SetScroll((ratio * max_scroll as f32).round() as i32),
            );
        }
    }

    let source_rect = Rect::new(
        x,
        rect.bottom() - 12 - support_h - source_h,
        inner_w,
        source_h,
    );
    draw_source_block(canvas, model, font, metrics, source_rect);
    let support_rect = Rect::new(x, source_rect.bottom(), inner_w, support_h);
    draw_support_block(canvas, font, metrics, support_rect);

    (action, max_scroll, anchor)
}

struct Sidebar<'a, 'c> {
    canvas: &'a mut Canvas<'c>,
    model: &'a Model,
    ui: &'a mut UiState,
    font: &'a Font,
    metrics: Metrics,
    action: Action,
    anchor: Option<Rect>,
    tip: &'a mut Tip,
    viewport: Rect,
}

impl Sidebar<'_, '_> {
    fn draw_rows(&mut self, x: i32, top: i32, width: i32, offset: i32) {
        let mut y = top - offset;
        for row in settings_rows(self.model) {
            let height = row_height(&row);
            let rect = Rect::new(x, y, width, height);
            if rect.bottom() > self.viewport.y && rect.y < self.viewport.bottom() {
                let fully_visible = self.viewport.intersect(rect) == rect;
                let saved_click = self.ui.click_pending;
                if !fully_visible {
                    self.ui.click_pending = false;
                }
                self.draw_row(&row, rect);
                if !fully_visible {
                    self.ui.click_pending = saved_click;
                }
            }
            y += height;
        }
    }

    fn draw_row(&mut self, row: &Row, rect: Rect) {
        match row {
            Row::Section(title) => {
                widgets::section_title(
                    self.canvas,
                    self.font,
                    rect.x,
                    rect.y + 8,
                    rect.w,
                    title,
                    self.metrics.body,
                );
            }
            Row::Number { field, label } => self.number_row(rect, *field, label),
            Row::Hex { field, label } => self.hex_row(rect, *field, label),
            Row::Toggle { toggle, label } => self.toggle_row(rect, *toggle, label),
            Row::Filter => self.filter_row(rect),
            Row::CornerColor => self.corner_color_row(rect),
            Row::Hint(text) => {
                self.font.draw(
                    self.canvas,
                    rect.x,
                    rect.y + 5,
                    text,
                    theme::MUTED,
                    self.metrics.small,
                );
            }
            Row::Gap(_) => {}
        }
    }

    fn number_row(&mut self, rect: Rect, field: Field, label: &str) {
        let enabled = self.model.field_enabled(field);
        let color = if enabled {
            theme::TEXT
        } else {
            theme::DISABLED_TEXT
        };
        let label_y = rect.y + (theme::ROW_H - Font::glyph_height(self.metrics.body)) / 2;
        self.font.draw(
            self.canvas,
            rect.x,
            label_y,
            label,
            color,
            self.metrics.body,
        );

        let y = rect.y + (theme::ROW_H - theme::INPUT_H) / 2;
        let total = theme::STEP_W * 2 + theme::FIELD_W + 4;
        let cx = rect.right() - total;
        let minus = Rect::new(cx, y, theme::STEP_W, theme::INPUT_H);
        let field_rect = Rect::new(cx + theme::STEP_W + 2, y, theme::FIELD_W, theme::INPUT_H);
        let plus = Rect::new(field_rect.right() + 2, y, theme::STEP_W, theme::INPUT_H);

        if widgets::step_button(
            self.canvas,
            self.font,
            minus,
            "-",
            enabled,
            self.ui,
            self.metrics.body,
        ) {
            self.action = Action::Step(field, -1);
        }
        let text = self.model.display_text(field);
        let state = InputState {
            text: &text,
            caret: self.model.display_caret(field),
            invalid: self.model.display_invalid(field),
            enabled,
            select_all: self.model.display_select_all(field),
        };
        if widgets::input(
            self.canvas,
            self.font,
            field_rect,
            &state,
            self.ui,
            self.metrics.body,
        ) {
            self.action = Action::BeginEdit(field);
        }
        if focused(self.model, FocusId::Field(field)) {
            widgets::focus_ring(self.canvas, field_rect);
        }
        hover_tip(self.ui, self.tip, rect, field_tip(field));
        if widgets::step_button(
            self.canvas,
            self.font,
            plus,
            "+",
            enabled,
            self.ui,
            self.metrics.body,
        ) {
            self.action = Action::Step(field, 1);
        }
    }

    fn hex_row(&mut self, rect: Rect, field: Field, label: &str) {
        let enabled = self.model.field_enabled(field);
        let color = if enabled {
            theme::TEXT
        } else {
            theme::DISABLED_TEXT
        };
        let label_y = rect.y + (theme::ROW_H - Font::glyph_height(self.metrics.body)) / 2;
        self.font.draw(
            self.canvas,
            rect.x,
            label_y,
            label,
            color,
            self.metrics.body,
        );

        let y = rect.y + (theme::ROW_H - theme::INPUT_H) / 2;
        let total = theme::SWATCH_W + 6 + theme::HEX_FIELD_W;
        let cx = rect.right() - total;
        let swatch = Rect::new(cx, y, theme::SWATCH_W, theme::INPUT_H);
        let field_rect = Rect::new(
            cx + theme::SWATCH_W + 6,
            y,
            theme::HEX_FIELD_W,
            theme::INPUT_H,
        );

        let rgb = self.model.field_color(field);
        let fill: Color = if enabled {
            [rgb[0], rgb[1], rgb[2], 255]
        } else {
            theme::DISABLED
        };
        self.canvas
            .fill_rect(swatch.x, swatch.y, swatch.w, swatch.h, fill);
        self.canvas
            .stroke_rect(swatch.x, swatch.y, swatch.w, swatch.h, theme::BORDER);
        if enabled && self.ui.clicked(swatch) {
            self.action = Action::BeginEdit(field);
        }

        let text = self.model.display_text(field);
        let state = InputState {
            text: &text,
            caret: self.model.display_caret(field),
            invalid: self.model.display_invalid(field),
            enabled,
            select_all: self.model.display_select_all(field),
        };
        if widgets::input(
            self.canvas,
            self.font,
            field_rect,
            &state,
            self.ui,
            self.metrics.body,
        ) {
            self.action = Action::BeginEdit(field);
        }
        if focused(self.model, FocusId::Field(field)) {
            widgets::focus_ring(self.canvas, field_rect);
        }
        hover_tip(self.ui, self.tip, rect, field_tip(field));
    }

    fn toggle_row(&mut self, rect: Rect, toggle: Toggle, label: &str) {
        let enabled = self.model.toggle_enabled(toggle);
        let checked = self.model.toggle_checked(toggle);
        let hovered = enabled && self.ui.hovered(rect);
        if hovered {
            self.canvas.fill_rect(
                rect.x - 4,
                rect.y + 2,
                rect.w + 8,
                theme::ROW_H - 4,
                theme::PANEL_HOVER,
            );
        }
        let box_rect = Rect::new(rect.x + 2, rect.y + (theme::ROW_H - 12) / 2, 12, 12);
        widgets::checkbox(self.canvas, box_rect, checked, enabled, hovered);
        let color = if enabled {
            theme::TEXT
        } else {
            theme::DISABLED_TEXT
        };
        let label_y = rect.y + (theme::ROW_H - Font::glyph_height(self.metrics.body)) / 2;
        self.font.draw(
            self.canvas,
            rect.x + 22,
            label_y,
            label,
            color,
            self.metrics.body,
        );
        if enabled && self.ui.clicked(rect) {
            self.action = Action::Toggle(toggle);
        }
        if focused(self.model, FocusId::Toggle(toggle)) {
            widgets::focus_ring(
                self.canvas,
                Rect::new(rect.x - 3, rect.y + 1, rect.w + 6, theme::ROW_H - 2),
            );
        }
        hover_tip(self.ui, self.tip, rect, toggle_tip(toggle));
    }

    fn filter_row(&mut self, rect: Rect) {
        let enabled = self.model.file.is_some();
        let color = if enabled {
            theme::TEXT
        } else {
            theme::DISABLED_TEXT
        };
        let label_y = rect.y + (theme::ROW_H - Font::glyph_height(self.metrics.body)) / 2;
        self.font.draw(
            self.canvas,
            rect.x,
            label_y,
            "ФИЛЬТР",
            color,
            self.metrics.body,
        );

        let y = rect.y + (theme::ROW_H - theme::INPUT_H) / 2;
        let control = Rect::new(
            rect.right() - theme::DROPDOWN_W,
            y,
            theme::DROPDOWN_W,
            theme::INPUT_H,
        );
        if widgets::dropdown_button(
            self.canvas,
            self.font,
            control,
            filter_label(self.model.params.filter),
            self.model.dropdown_open,
            enabled,
            self.ui,
            self.metrics.body,
        ) {
            self.action = Action::ToggleDropdown;
        }
        if focused(self.model, FocusId::Filter) {
            widgets::focus_ring(self.canvas, control);
        }
        self.anchor = Some(control);
    }

    /// "ВЗЯТЬ ЦВЕТ С УГЛА": copies the top-left source pixel into the manual
    /// background colour and switches the auto colour off.
    fn corner_color_row(&mut self, rect: Rect) {
        let enabled = self.model.file.is_some();
        let label = "ВЗЯТЬ ЦВЕТ С УГЛА";
        let width = (self.font.width(label, self.metrics.body) + 26)
            .min(rect.w)
            .max(140);
        let button = Rect::new(
            rect.right() - width,
            rect.y + (rect.h - theme::INPUT_H) / 2,
            width,
            theme::INPUT_H,
        );
        if widgets::button_tone(
            self.canvas,
            self.font,
            button,
            label,
            enabled,
            self.ui,
            self.metrics.body,
            ButtonTone::Secondary,
        ) {
            self.action = Action::PickCornerColor;
        }
        if focused(self.model, FocusId::PickCornerColor) {
            widgets::focus_ring(self.canvas, button);
        }
        hover_tip(
            self.ui,
            self.tip,
            rect,
            "ВЗЯТЬ ЦВЕТ ПИКСЕЛЯ (0,0) И ВКЛЮЧИТЬ РУЧНОЙ ЦВЕТ ФОНА",
        );
    }
}

enum Row {
    Section(&'static str),
    Number {
        field: Field,
        label: &'static str,
    },
    Hex {
        field: Field,
        label: &'static str,
    },
    Toggle {
        toggle: Toggle,
        label: &'static str,
    },
    Filter,
    /// "ВЗЯТЬ ЦВЕТ С УГЛА" button of the background block.
    CornerColor,
    Hint(&'static str),
    Gap(i32),
}

fn row_height(row: &Row) -> i32 {
    match row {
        Row::Section(_) => 28,
        Row::Number { .. } | Row::Hex { .. } | Row::Toggle { .. } | Row::Filter => theme::ROW_H,
        Row::CornerColor => theme::ROW_H,
        Row::Hint(_) => 16,
        Row::Gap(height) => *height,
    }
}

fn settings_rows(model: &Model) -> Vec<Row> {
    match model.mode {
        Mode::PixelArt => pixel_rows(&model.params),
        Mode::Cutout => cutout_rows(),
    }
}

/// Visual order of the keyboard-focusable controls (Tab / Shift+Tab).
///
/// Built from the same rows the sidebar draws, so focus never lands on a
/// control that is not on screen.
pub fn focus_order(model: &Model) -> Vec<FocusId> {
    // The export dialog is modal: Tab only walks its four controls.
    if model.export_dialog_open() {
        return vec![
            FocusId::Field(Field::OutputPrefix),
            FocusId::PickOutputDir,
            FocusId::ExportSave,
            FocusId::ExportCancel,
        ];
    }
    let mut order = Vec::new();
    for mode in Mode::ALL {
        order.push(FocusId::ModeTab(mode));
    }
    order.push(FocusId::Open);
    if model.file.is_some() || model.is_busy() {
        order.push(FocusId::Clear);
    }
    if model.can_generate() {
        order.push(FocusId::Generate);
    }
    if model.can_save() {
        order.push(FocusId::Save);
        order.push(FocusId::Export);
    }
    order.push(FocusId::ResetSettings);
    if model.can_undo() {
        order.push(FocusId::Undo);
    }
    if model.can_redo() {
        order.push(FocusId::Redo);
    }
    if model.file.is_some() && model.mode == Mode::PixelArt {
        order.push(FocusId::Filter);
    }
    for row in settings_rows(model) {
        match row {
            Row::Number { field, .. } | Row::Hex { field, .. } => {
                if model.field_enabled(field) {
                    order.push(FocusId::Field(field));
                }
            }
            Row::Toggle { toggle, .. } => {
                if model.toggle_enabled(toggle) {
                    order.push(FocusId::Toggle(toggle));
                }
            }
            Row::CornerColor if model.file.is_some() => {
                order.push(FocusId::PickCornerColor);
            }
            _ => {}
        }
    }
    order.push(FocusId::ZoomOut);
    order.push(FocusId::ZoomIn);
    order.push(FocusId::ZoomFit);
    if model.file.is_none() {
        for index in 0..model.recent.len().min(RECENT_VISIBLE) {
            order.push(FocusId::Recent(index));
        }
    }
    order
}

fn pixel_rows(params: &PixelParams) -> Vec<Row> {
    let mut rows = vec![
        Row::Section("ПИКСЕЛЬ-АРТ"),
        Row::Number {
            field: Field::Width,
            label: "ШИРИНА",
        },
        Row::Filter,
        Row::Hint(filter_hint(params.filter)),
        Row::Number {
            field: Field::Colors,
            label: "ЦВЕТА",
        },
        Row::Number {
            field: Field::Cell,
            label: "ЯЧЕЙКА",
        },
        Row::Number {
            field: Field::GridOpacity,
            label: "СЕТКА, %",
        },
        Row::Hex {
            field: Field::GridColor,
            label: "ЦВЕТ СЕТКИ",
        },
        Row::Number {
            field: Field::AlphaThreshold,
            label: "ПОРОГ АЛЬФЫ",
        },
        Row::Toggle {
            toggle: Toggle::Fill,
            label: "ЗАЛИВКА ФОНА",
        },
    ];
    if params.fill_enabled {
        rows.push(Row::Hex {
            field: Field::FillColor,
            label: "ЦВЕТ ЗАЛИВКИ",
        });
    }
    rows.push(Row::Number {
        field: Field::Despeckle,
        label: "МУСОР",
    });
    rows.push(Row::Gap(8));
    rows.push(Row::Section("УДАЛЕНИЕ ФОНА"));
    rows.push(Row::Toggle {
        toggle: Toggle::RemoveBg,
        label: "УДАЛИТЬ ФОН",
    });
    if params.remove_bg {
        rows.push(Row::Toggle {
            toggle: Toggle::BgAutoColor,
            label: "ЦВЕТ ФОНА АВТО",
        });
        rows.push(Row::Hex {
            field: Field::BgColor,
            label: "ЦВЕТ ФОНА",
        });
        rows.push(Row::CornerColor);
        rows.push(Row::Number {
            field: Field::BgTolerance,
            label: "ДОПУСК",
        });
        rows.push(Row::Number {
            field: Field::BgGrow,
            label: "КАЙМА, PX",
        });
        rows.push(Row::Toggle {
            toggle: Toggle::BgAutoDefringe,
            label: "КРАЙ АВТО",
        });
        rows.push(Row::Number {
            field: Field::BgDefringe,
            label: "КРАЙ, PX",
        });
        rows.push(Row::Toggle {
            toggle: Toggle::ThinGaps,
            label: "ТОНКИЕ ЩЕЛИ",
        });
        rows.push(Row::Toggle {
            toggle: Toggle::RemoveHoles,
            label: "ПРОСВЕТЫ",
        });
        rows.push(Row::Number {
            field: Field::HoleArea,
            label: "ПЛОЩАДЬ, %",
        });
    } else {
        rows.push(Row::Hint("ВЫРЕЗАЕТ ОДНОТОННЫЙ ФОН JPG/PNG"));
    }
    rows
}

/// Cutout mode: pixel controls are hidden, the background block is always on.
fn cutout_rows() -> Vec<Row> {
    vec![
        Row::Section("ВЫРЕЗКА ФОНА"),
        Row::Hint("РЕЗУЛЬТАТ В ИСХОДНОМ РАЗМЕРЕ"),
        Row::Toggle {
            toggle: Toggle::BgAutoColor,
            label: "ЦВЕТ ФОНА АВТО",
        },
        Row::Hex {
            field: Field::BgColor,
            label: "ЦВЕТ ФОНА",
        },
        Row::CornerColor,
        Row::Number {
            field: Field::BgTolerance,
            label: "ДОПУСК",
        },
        Row::Number {
            field: Field::BgGrow,
            label: "КАЙМА, PX",
        },
        Row::Toggle {
            toggle: Toggle::BgAutoDefringe,
            label: "КРАЙ АВТО",
        },
        Row::Number {
            field: Field::BgDefringe,
            label: "КРАЙ, PX",
        },
        Row::Toggle {
            toggle: Toggle::ThinGaps,
            label: "ТОНКИЕ ЩЕЛИ",
        },
        Row::Toggle {
            toggle: Toggle::RemoveHoles,
            label: "ПРОСВЕТЫ",
        },
        Row::Number {
            field: Field::HoleArea,
            label: "ПЛОЩАДЬ, %",
        },
        Row::Gap(8),
        Row::Hint("ПИКСЕЛЬНЫЕ НАСТРОЙКИ НЕ ИСПОЛЬЗУЮТСЯ"),
    ]
}

fn draw_source_block(
    canvas: &mut Canvas,
    model: &Model,
    font: &Font,
    metrics: Metrics,
    rect: Rect,
) {
    if rect.is_empty() {
        return;
    }
    canvas.hline(rect.x, rect.y, rect.w, theme::BORDER);
    let mut y = rect.y + 10;
    widgets::section_title(canvas, font, rect.x, y, rect.w, "ИСТОЧНИК", metrics.body);
    y += Font::glyph_height(metrics.body) + 10;

    match &model.file {
        Some(file) => {
            let name_lines = wrap_limit(font, &file.file_name(), rect.w, metrics.body, 2);
            for line in name_lines {
                font.draw(canvas, rect.x, y, &line, theme::TEXT, metrics.body);
                y += Font::line_height(metrics.body);
            }
            y += 2;
            let dimensions = format!("{} X {}", file.meta.width, file.meta.height);
            font.draw(canvas, rect.x, y, &dimensions, theme::TEXT, metrics.body);
            y += Font::line_height(metrics.body) + 2;
            let details = format!("{} / {}", file.meta.format, human_bytes(file.meta.bytes));
            font.draw(canvas, rect.x, y, &details, theme::MUTED, metrics.body);
            y += Font::line_height(metrics.body) + 4;
            let path = file.path.display().to_string();
            let path_lines = wrap_limit(font, &path, rect.w, metrics.small, 2);
            for line in path_lines {
                font.draw(canvas, rect.x, y, &line, theme::MUTED, metrics.small);
                y += Font::line_height(metrics.small);
            }
        }
        None => {
            if let Some(job) = &model.loading {
                font.draw(canvas, rect.x, y, "ЗАГРУЗКА...", theme::TEXT, metrics.body);
                y += Font::line_height(metrics.body) + 2;
                let name = job
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| job.path.display().to_string());
                let lines = wrap_limit(font, &name, rect.w, metrics.small, 2);
                for line in lines {
                    font.draw(canvas, rect.x, y, &line, theme::MUTED, metrics.small);
                    y += Font::line_height(metrics.small);
                }
            } else {
                font.draw(
                    canvas,
                    rect.x,
                    y,
                    "ФАЙЛ НЕ ВЫБРАН",
                    theme::TEXT,
                    metrics.body,
                );
                y += Font::line_height(metrics.body);
                font.draw(
                    canvas,
                    rect.x,
                    y,
                    "ОТКРОЙТЕ ИЗОБРАЖЕНИЕ",
                    theme::MUTED,
                    metrics.small,
                );
            }
        }
    }
}

fn draw_support_block(canvas: &mut Canvas, font: &Font, metrics: Metrics, rect: Rect) {
    if rect.is_empty() {
        return;
    }
    canvas.hline(rect.x, rect.y, rect.w, theme::BORDER);
    font.draw(
        canvas,
        rect.x,
        rect.y + 8,
        "ПОДДЕРЖКА: PNG JPG WEBP BMP GIF",
        theme::MUTED,
        metrics.small,
    );
}

/// Layout facts of the preview panel.
struct PreviewInfo {
    action: Action,
    /// Image area (below the header), used to hit-test pan dragging.
    body: Rect,
    /// Largest pan offset the current zoom allows, per axis.
    pan_limit: Point,
    /// Measured fit percentage of the primary image.
    fit_percent: Option<u32>,
}

impl PreviewInfo {
    fn new(body: Rect) -> Self {
        Self {
            action: Action::None,
            body,
            pan_limit: Point::new(0, 0),
            fit_percent: None,
        }
    }

    fn observe(&mut self, drawn: PaneInfo) {
        self.pan_limit = Point::new(
            self.pan_limit.x.max(drawn.limit.0),
            self.pan_limit.y.max(drawn.limit.1),
        );
        self.fit_percent = Some(drawn.fit_percent);
    }
}

/// Result of drawing one preview pane.
struct PaneInfo {
    /// Area the image was fitted into (also the busy overlay target).
    area: Rect,
    limit: (i32, i32),
    fit_percent: u32,
}

/// Zoom and pan shared by the source and result panes.
#[derive(Clone, Copy)]
struct PreviewZoom {
    mode: ZoomMode,
    pan: Point,
}

impl PreviewZoom {
    fn new(model: &Model) -> Self {
        Self {
            mode: model.zoom,
            pan: model.pan,
        }
    }

    /// Destination rectangle of the image inside `area`.
    fn dst(self, src_w: u32, src_h: u32, area: Rect) -> Rect {
        match self.mode {
            ZoomMode::Fit => fit_rect(src_w, src_h, area, true),
            ZoomMode::Percent(percent) => {
                let scale = percent as f32 / 100.0;
                let w = ((src_w as f32 * scale).round() as i32).max(1);
                let h = ((src_h as f32 * scale).round() as i32).max(1);
                let (max_x, max_y) = self.limit(src_w, src_h, area);
                let pan_x = self.pan.x.clamp(-max_x, max_x);
                let pan_y = self.pan.y.clamp(-max_y, max_y);
                Rect::new(
                    area.x + (area.w - w) / 2 + pan_x,
                    area.y + (area.h - h) / 2 + pan_y,
                    w,
                    h,
                )
            }
        }
    }

    /// How far the content may be dragged before its edge leaves the pane.
    fn limit(self, src_w: u32, src_h: u32, area: Rect) -> (i32, i32) {
        match self.mode {
            ZoomMode::Fit => (0, 0),
            ZoomMode::Percent(percent) => {
                let scale = percent as f32 / 100.0;
                let w = ((src_w as f32 * scale).round() as i32).max(1);
                let h = ((src_h as f32 * scale).round() as i32).max(1);
                (((w - area.w).max(0) + 1) / 2, ((h - area.h).max(0) + 1) / 2)
            }
        }
    }
}

/// Percentage of the image visible when fitted into `area`.
fn fit_percent(src_w: u32, src_h: u32, area: Rect) -> u32 {
    if src_w == 0 || src_h == 0 || area.is_empty() {
        return 100;
    }
    let scale = (area.w as f32 / src_w as f32).min(area.h as f32 / src_h as f32);
    ((scale * 100.0).round() as u32).clamp(1, 10_000)
}

/// Draw an RGBA image with the current zoom, clipped to the pane `area`.
fn draw_zoom_image(
    canvas: &mut Canvas,
    rgba: &[u8],
    src_w: u32,
    src_h: u32,
    area: Rect,
    dst: Rect,
    fill: Option<Color>,
) {
    if area.is_empty() || dst.is_empty() {
        return;
    }
    canvas.set_clip(area);
    match fill {
        Some(fill) => {
            canvas.fill_rect(dst.x, dst.y, dst.w, dst.h, fill);
            canvas.draw_rgba_nearest_over(rgba, src_w, src_h, dst, fill);
        }
        None => {
            canvas.fill_checker_rect(dst.x, dst.y, dst.w, dst.h);
            canvas.draw_rgba_nearest(rgba, src_w, src_h, dst, true);
        }
    }
    canvas.stroke_rect(dst.x - 1, dst.y - 1, dst.w + 2, dst.h + 2, theme::BORDER);
    canvas.clear_clip();
}

#[allow(clippy::too_many_arguments)]
fn draw_preview(
    canvas: &mut Canvas,
    model: &Model,
    ui: &mut UiState,
    font: &Font,
    metrics: Metrics,
    rect: Rect,
    tip: &mut Tip,
) -> PreviewInfo {
    if rect.is_empty() {
        return PreviewInfo::new(rect);
    }
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, theme::WHITE);
    canvas.stroke_rect(rect.x, rect.y, rect.w, rect.h, theme::BORDER);

    let header = Rect::new(rect.x + 1, rect.y + 1, rect.w - 2, PREVIEW_HEADER_H);
    canvas.fill_rect(header.x, header.y, header.w, header.h, theme::PANEL_ALT);
    canvas.hline(header.x, header.bottom(), header.w, theme::BORDER);
    let title_y = header.y + (header.h - Font::glyph_height(metrics.body)) / 2;
    font.draw(
        canvas,
        header.x + 12,
        title_y,
        "ПРЕДПРОСМОТР",
        theme::TEXT,
        metrics.body,
    );

    // Zoom chips, right aligned. The percentage badge is painted after the
    // panes are drawn: its value is the measured fit percentage.
    let small = metrics.small;
    let fit_label = "ПО РАЗМЕРУ";
    let fit_w = (font.width(fit_label, small) + 20).max(84);
    let badge_w = 54;
    let step_w = 22;
    let group_w = step_w + 4 + badge_w + 4 + step_w + 10 + fit_w;
    let group_x = header.right() - 12 - group_w;
    let chip_y = header.y + (header.h - CHIP_H) / 2;
    let minus = Rect::new(group_x, chip_y, step_w, CHIP_H);
    let badge = Rect::new(minus.right() + 4, chip_y, badge_w, CHIP_H);
    let plus = Rect::new(badge.right() + 4, chip_y, step_w, CHIP_H);
    let fit = Rect::new(plus.right() + 10, chip_y, fit_w, CHIP_H);

    let mut action = Action::None;
    if widgets::chip(canvas, font, minus, "-", false, true, ui, small) {
        action = Action::ZoomOut;
    }
    if focused(model, FocusId::ZoomOut) {
        widgets::focus_ring(canvas, minus);
    }
    hover_tip(
        ui,
        tip,
        minus,
        "УМЕНЬШИТЬ МАСШТАБ (CTRL+МИНУС, КОЛЕСО МЫШИ)",
    );

    if widgets::chip(canvas, font, plus, "+", false, true, ui, small) {
        action = Action::ZoomIn;
    }
    if focused(model, FocusId::ZoomIn) {
        widgets::focus_ring(canvas, plus);
    }
    hover_tip(ui, tip, plus, "УВЕЛИЧИТЬ МАСШТАБ (CTRL+ПЛЮС, КОЛЕСО МЫШИ)");

    let fit_active = model.zoom == ZoomMode::Fit;
    if widgets::chip(canvas, font, fit, fit_label, fit_active, true, ui, small) {
        action = Action::ZoomFit;
    }
    if focused(model, FocusId::ZoomFit) {
        widgets::focus_ring(canvas, fit);
    }
    hover_tip(ui, tip, fit, "ВПИСАТЬ В ОКНО (CTRL+0)");

    let size_label = match model.mode {
        Mode::PixelArt => match (&model.pixel_result, &model.file) {
            (Some(result), _) => {
                let (width, height) = result.output_dims();
                format!("{width} X {height}")
            }
            (None, Some(file)) => format!("{} X {}", file.meta.width, file.meta.height),
            (None, None) => String::new(),
        },
        Mode::Cutout => match (&model.cutout_result, &model.file) {
            (Some(result), _) => format!("{} X {}", result.width, result.height),
            (None, Some(file)) => format!("{} X {}", file.meta.width, file.meta.height),
            (None, None) => String::new(),
        },
    };
    if !size_label.is_empty() {
        let label_right = group_x - 12;
        let title_right = header.x + 12 + font.width("ПРЕДПРОСМОТР", metrics.body);
        if label_right > title_right + 16 {
            font.draw_right(
                canvas,
                label_right,
                header.y + (header.h - Font::glyph_height(small)) / 2,
                &size_label,
                theme::MUTED,
                small,
            );
        }
    }

    let body = Rect::new(
        rect.x + 12,
        header.bottom() + 12,
        rect.w - 24,
        rect.bottom() - header.bottom() - 24,
    );
    let zoom = PreviewZoom::new(model);
    let mut info = PreviewInfo::new(body);
    let mut busy_area = body;

    if let Some(file) = &model.file {
        let split = match model.mode {
            Mode::PixelArt => model.pixel_result.is_some(),
            Mode::Cutout => model.cutout_result.is_some(),
        };
        let (left_pane, right_pane) = if split {
            split_panes(body)
        } else {
            (body, body)
        };
        match model.mode {
            Mode::PixelArt => match &model.pixel_result {
                None => info.observe(draw_source_fit(canvas, file, body, zoom)),
                Some(result) => {
                    info.observe(draw_source_pane(
                        canvas, font, metrics, left_pane, file, zoom,
                    ));
                    let drawn = draw_result_pane(canvas, font, metrics, right_pane, result, zoom);
                    busy_area = drawn.area;
                    info.observe(drawn);
                }
            },
            Mode::Cutout => match &model.cutout_result {
                None => info.observe(draw_source_fit(canvas, file, body, zoom)),
                Some(result) => {
                    info.observe(draw_source_pane(
                        canvas, font, metrics, left_pane, file, zoom,
                    ));
                    let drawn = draw_cutout_pane(canvas, font, metrics, right_pane, result, zoom);
                    busy_area = drawn.area;
                    info.observe(drawn);
                }
            },
        }
    } else if !model.is_busy() {
        // While the first file is loading, the overlay owns the area; do not
        // leave an invisible, clickable button underneath it.
        action = draw_empty_state(canvas, model, ui, font, metrics, body);
    }
    info.action = action;

    widgets::badge(
        canvas,
        font,
        badge,
        &model.zoom.label(info.fit_percent),
        small,
    );
    hover_tip(
        ui,
        tip,
        badge,
        "ТЕКУЩИЙ МАСШТАБ; КАРТИНКУ МОЖНО ПЕРЕТАСКИВАТЬ МЫШЬЮ",
    );

    if model.is_generating() {
        draw_busy(canvas, ui, font, metrics, busy_area, "ГЕНЕРАЦИЯ");
    } else if model.is_cutting() {
        draw_busy(canvas, ui, font, metrics, busy_area, "ВЫРЕЗКА");
    } else if model.is_saving() {
        let label = match model.saving_target() {
            Some(SaveTarget::Export) => "ЭКСПОРТ",
            _ => "СОХРАНЕНИЕ",
        };
        draw_busy(canvas, ui, font, metrics, busy_area, label);
    } else if model.loading.is_some() {
        draw_busy(canvas, ui, font, metrics, body, "ЗАГРУЗКА");
    }

    info
}

fn split_panes(body: Rect) -> (Rect, Rect) {
    let pane_w = (body.w - GAP).max(0) / 2;
    let left = Rect::new(body.x, body.y, pane_w, body.h);
    let right = Rect::new(body.x + pane_w + GAP, body.y, body.w - pane_w - GAP, body.h);
    (left, right)
}

/// Source image fitted into the whole preview body (no result yet).
fn draw_source_fit(
    canvas: &mut Canvas,
    file: &crate::model::LoadedFile,
    area: Rect,
    zoom: PreviewZoom,
) -> PaneInfo {
    let (src_w, src_h) = file.image.dimensions();
    let dst = zoom.dst(src_w, src_h, area);
    draw_zoom_image(canvas, file.image.as_raw(), src_w, src_h, area, dst, None);
    PaneInfo {
        area,
        limit: zoom.limit(src_w, src_h, area),
        fit_percent: fit_percent(src_w, src_h, area),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_source_pane(
    canvas: &mut Canvas,
    font: &Font,
    metrics: Metrics,
    pane: Rect,
    file: &crate::model::LoadedFile,
    zoom: PreviewZoom,
) -> PaneInfo {
    if pane.is_empty() {
        return PaneInfo {
            area: pane,
            limit: (0, 0),
            fit_percent: 100,
        };
    }
    canvas.fill_rect(pane.x, pane.y, pane.w, pane.h, theme::WHITE);
    canvas.stroke_rect(pane.x, pane.y, pane.w, pane.h, theme::BORDER);
    let header = Rect::new(pane.x + 1, pane.y + 1, pane.w - 2, 24);
    canvas.fill_rect(header.x, header.y, header.w, header.h, theme::PANEL_ALT);
    canvas.hline(header.x, header.bottom(), header.w, theme::BORDER);
    let y = header.y + (header.h - Font::glyph_height(metrics.body)) / 2;
    font.draw(
        canvas,
        header.x + 10,
        y,
        "ИСХОДНОЕ",
        theme::TEXT,
        metrics.body,
    );
    let label = format!("{} X {}", file.meta.width, file.meta.height);
    font.draw_right(
        canvas,
        header.right() - 10,
        header.y + (header.h - Font::glyph_height(metrics.small)) / 2,
        &label,
        theme::MUTED,
        metrics.small,
    );

    let area = Rect::new(
        pane.x + 10,
        header.bottom() + 10,
        pane.w - 20,
        pane.bottom() - header.bottom() - 20,
    );
    let (src_w, src_h) = file.image.dimensions();
    let dst = zoom.dst(src_w, src_h, area);
    draw_zoom_image(canvas, file.image.as_raw(), src_w, src_h, area, dst, None);
    PaneInfo {
        area,
        limit: zoom.limit(src_w, src_h, area),
        fit_percent: fit_percent(src_w, src_h, area),
    }
}

fn draw_result_pane(
    canvas: &mut Canvas,
    font: &Font,
    metrics: Metrics,
    pane: Rect,
    result: &GeneratedImage,
    zoom: PreviewZoom,
) -> PaneInfo {
    if pane.is_empty() {
        return PaneInfo {
            area: pane,
            limit: (0, 0),
            fit_percent: 100,
        };
    }
    canvas.fill_rect(pane.x, pane.y, pane.w, pane.h, theme::WHITE);
    canvas.stroke_rect(pane.x, pane.y, pane.w, pane.h, theme::BORDER);
    let header = Rect::new(pane.x + 1, pane.y + 1, pane.w - 2, 24);
    canvas.fill_rect(header.x, header.y, header.w, header.h, theme::PANEL_ALT);
    canvas.hline(header.x, header.bottom(), header.w, theme::BORDER);
    let y = header.y + (header.h - Font::glyph_height(metrics.body)) / 2;
    font.draw(
        canvas,
        header.x + 10,
        y,
        "РЕЗУЛЬТАТ",
        theme::TEXT,
        metrics.body,
    );
    let (out_w, out_h) = result.output_dims();
    let label = format!(
        "{} X {} | {} ЦВЕТОВ",
        out_w, out_h, result.stats.palette_colors
    );
    font.draw_right(
        canvas,
        header.right() - 10,
        header.y + (header.h - Font::glyph_height(metrics.small)) / 2,
        &label,
        theme::MUTED,
        metrics.small,
    );

    let area = Rect::new(
        pane.x + 10,
        header.bottom() + 10,
        pane.w - 20,
        pane.bottom() - header.bottom() - 20,
    );
    let (sw, sh) = result.sprite.dimensions();
    let dst = zoom.dst(sw, sh, area);
    let fill = result
        .params
        .fill_enabled
        .then(|| to_rgba(result.params.fill_color));
    draw_zoom_image(canvas, result.sprite.as_raw(), sw, sh, area, dst, fill);
    canvas.set_clip(area);
    draw_grid_overlay(
        canvas,
        dst,
        result,
        result.params.grid_color,
        result.params.grid_opacity,
    );
    canvas.clear_clip();
    draw_sprite_card(canvas, font, metrics, area, result);

    PaneInfo {
        area,
        limit: zoom.limit(sw, sh, area),
        fit_percent: fit_percent(sw, sh, area),
    }
}

/// Cutout result pane: the on-screen proxy plus the core `CleanStats` in a
/// footer. The export always uses the full-resolution image.
fn draw_cutout_pane(
    canvas: &mut Canvas,
    font: &Font,
    metrics: Metrics,
    pane: Rect,
    result: &CutoutImage,
    zoom: PreviewZoom,
) -> PaneInfo {
    if pane.is_empty() {
        return PaneInfo {
            area: pane,
            limit: (0, 0),
            fit_percent: 100,
        };
    }
    canvas.fill_rect(pane.x, pane.y, pane.w, pane.h, theme::WHITE);
    canvas.stroke_rect(pane.x, pane.y, pane.w, pane.h, theme::BORDER);
    let header = Rect::new(pane.x + 1, pane.y + 1, pane.w - 2, 24);
    canvas.fill_rect(header.x, header.y, header.w, header.h, theme::PANEL_ALT);
    canvas.hline(header.x, header.bottom(), header.w, theme::BORDER);
    let y = header.y + (header.h - Font::glyph_height(metrics.body)) / 2;
    font.draw(
        canvas,
        header.x + 10,
        y,
        "РЕЗУЛЬТАТ",
        theme::TEXT,
        metrics.body,
    );
    let label = if result.is_downscaled() {
        format!("{} X {} | ПРЕВЬЮ", result.width, result.height)
    } else {
        format!("{} X {}", result.width, result.height)
    };
    font.draw_right(
        canvas,
        header.right() - 10,
        header.y + (header.h - Font::glyph_height(metrics.small)) / 2,
        &label,
        theme::MUTED,
        metrics.small,
    );

    // Stats footer: wrapped small lines, the warning first and in red.
    let mut lines: Vec<(String, bool)> = Vec::new();
    let wrap_width = (pane.w - 36).max(80);
    for (text, warning) in cutout_stat_lines(result) {
        for part in wrap_limit(font, &text, wrap_width, metrics.small, 2) {
            if lines.len() >= 6 {
                break;
            }
            lines.push((part, warning));
        }
    }
    let line_h = Font::line_height(metrics.small);
    let footer_h = if lines.is_empty() {
        0
    } else {
        lines.len() as i32 * line_h + 12
    };
    let footer = Rect::new(
        pane.x + 1,
        pane.bottom() - 1 - footer_h,
        pane.w - 2,
        footer_h,
    );
    if footer_h > 0 {
        canvas.fill_rect(footer.x, footer.y, footer.w, footer.h, theme::PANEL_ALT);
        canvas.hline(footer.x, footer.y, footer.w, theme::BORDER);
    }

    let area = Rect::new(
        pane.x + 10,
        header.bottom() + 10,
        pane.w - 20,
        (footer.y - header.bottom() - 20).max(0),
    );
    let (dw, dh) = result.display_dims();
    let dst = zoom.dst(dw, dh, area);
    draw_zoom_image(canvas, result.display.as_raw(), dw, dh, area, dst, None);

    let mut text_y = footer.y + 6;
    for (text, warning) in &lines {
        let color = if *warning { theme::RED } else { theme::MUTED };
        font.draw(canvas, footer.x + 10, text_y, text, color, metrics.small);
        text_y += line_h;
    }

    PaneInfo {
        area,
        limit: zoom.limit(dw, dh, area),
        fit_percent: fit_percent(dw, dh, area),
    }
}

/// Human-readable `CleanStats`, in the same order as the CLI report.
fn cutout_stat_lines(result: &CutoutImage) -> Vec<(String, bool)> {
    let mut lines = Vec::new();
    if result.fully_transparent {
        lines.push((
            "ОБЪЕКТ НЕ НАЙДЕН: РЕЗУЛЬТАТ ПОЛНОСТЬЮ ПРОЗРАЧНЫЙ".to_string(),
            true,
        ));
    }
    if let Some(background) = &result.stats.background {
        let [r, g, b] = background.color;
        lines.push((
            format!(
                "ФОН #{r:02X}{g:02X}{b:02X}: УДАЛЕНО {} PX ({:.1}%)",
                background.removed, background.removed_percent
            ),
            false,
        ));
        lines.push((
            format!(
                "МАТИРИРОВАНИЕ: ПОЛОСА {} PX, ИЗМЕНЕНО {} PX",
                background.defringe_depth, background.defringe_changed
            ),
            false,
        ));
        if background.thin_gaps_removed > 0 {
            lines.push((
                format!("ТОНКИЕ ЩЕЛИ: {} PX", background.thin_gaps_removed),
                false,
            ));
        }
    }
    if result.stats.faint_alpha_removed > 0 {
        lines.push((
            format!("СЛАБАЯ АЛЬФА: {} PX", result.stats.faint_alpha_removed),
            false,
        ));
    }
    lines
}

fn draw_grid_overlay(
    canvas: &mut Canvas,
    dst: Rect,
    result: &GeneratedImage,
    color: CoreColor,
    opacity: u8,
) {
    if opacity == 0 || dst.is_empty() {
        return;
    }
    let (sw, sh) = result.sprite.dimensions();
    if sw == 0 || sh == 0 {
        return;
    }
    let cell_w = dst.w as f32 / sw as f32;
    let cell_h = dst.h as f32 / sh as f32;
    let scale = canvas.scale() as f32;
    // Below ~2 physical px per cell the lines would swallow the pixels. §5.4
    // says 2 px and §5.8.4/§5.14 say 3 px; the code keeps 2 so the default
    // "по размеру" view still shows the grid (see SPEC 5.19).
    if cell_w * scale < 2.0 || cell_h * scale < 2.0 {
        return;
    }
    let alpha = ((opacity as u32 * 255 + 50) / 100) as u8;
    let line = [color[0], color[1], color[2], alpha];
    let raw = result.sprite.as_raw();
    // With a fill colour the core flattens every transparent cell before
    // drawing the grid, so the exported grid has a full lattice.
    let filled = result.params.fill_enabled;
    let solid = |x: i64, y: i64| {
        x >= 0
            && y >= 0
            && x < sw as i64
            && y < sh as i64
            && (filled || raw[((y as u32 * sw + x as u32) * 4 + 3) as usize] != 0)
    };
    // When zoomed in only a few cells are visible: skip the rest instead of
    // blending every line of the whole sprite.
    let clip = canvas.clip();
    for gy in 0..sh {
        let yi = gy as i64;
        let y0 = dst.y + (yi as f32 * cell_h).round() as i32;
        let y1 = dst.y + ((yi + 1) as f32 * cell_h).round() as i32;
        if y1 < clip.y || y0 > clip.bottom() {
            continue;
        }
        for gx in 0..sw {
            let xi = gx as i64;
            let x0 = dst.x + (gx as f32 * cell_w).round() as i32;
            let x1 = dst.x + ((gx + 1) as f32 * cell_w).round() as i32;
            if x1 < clip.x || x0 > clip.right() {
                continue;
            }
            let me = solid(xi, yi);
            let v_edge = me || solid(xi - 1, yi);
            let h_edge = me || solid(xi, yi - 1);
            let corner = v_edge || h_edge || solid(xi - 1, yi - 1);
            if !me && !corner {
                continue;
            }
            if v_edge {
                for y in y0..y1.max(y0 + 1) {
                    blend_pixel(canvas, x0, y, line);
                }
            }
            if h_edge {
                for x in x0..x1.max(x0 + 1) {
                    blend_pixel(canvas, x, y0, line);
                }
            }
        }
    }
}

fn blend_pixel(canvas: &mut Canvas, x: i32, y: i32, color: Color) {
    let back = canvas.pixel(x, y);
    let blended = over(color, back);
    canvas.fill_rect(x, y, 1, 1, blended);
}

fn draw_sprite_card(
    canvas: &mut Canvas,
    font: &Font,
    metrics: Metrics,
    area: Rect,
    result: &GeneratedImage,
) {
    let card_w = theme::SPRITE_CARD_W;
    let card_h = theme::SPRITE_CARD_H;
    if area.w < card_w + 80 || area.h < card_h + 80 {
        return;
    }
    let card = Rect::new(
        area.right() - card_w - 10,
        area.bottom() - card_h - 10,
        card_w,
        card_h,
    );
    canvas.fill_rect(
        card.x + theme::SHADOW,
        card.y + theme::SHADOW,
        card.w,
        card.h,
        theme::BLACK,
    );
    canvas.fill_rect(card.x, card.y, card.w, card.h, theme::WHITE);
    canvas.stroke_rect(card.x, card.y, card.w, card.h, theme::BORDER);
    font.draw(
        canvas,
        card.x + 10,
        card.y + 9,
        "СПРАЙТ",
        theme::TEXT,
        metrics.body,
    );
    let (sw, sh) = result.sprite.dimensions();
    font.draw(
        canvas,
        card.x + 10,
        card.y + 27,
        &format!("{sw} X {sh}"),
        theme::MUTED,
        metrics.small,
    );
    let sprite_area = Rect::new(card.x + 10, card.y + 40, card.w - 20, card.h - 50);
    let dst = fit_rect(sw, sh, sprite_area, true);
    if !dst.is_empty() {
        if result.params.fill_enabled {
            let fill = to_rgba(result.params.fill_color);
            canvas.fill_rect(dst.x, dst.y, dst.w, dst.h, fill);
            canvas.draw_rgba_nearest_over(result.sprite.as_raw(), sw, sh, dst, fill);
        } else {
            canvas.fill_checker_rect(dst.x, dst.y, dst.w, dst.h);
            canvas.draw_rgba_nearest(result.sprite.as_raw(), sw, sh, dst, true);
        }
        canvas.stroke_rect(dst.x - 1, dst.y - 1, dst.w + 2, dst.h + 2, theme::BORDER);
    }
}

fn draw_empty_state(
    canvas: &mut Canvas,
    model: &Model,
    ui: &mut UiState,
    font: &Font,
    metrics: Metrics,
    area: Rect,
) -> Action {
    let icon_size = 64;
    let icon_area = Rect::new(area.x, area.center_y() - 92, area.w, icon_size);
    icons::image_icon(canvas, icon_area, 4, theme::BLUE);

    font.draw_centered(
        canvas,
        area.center_x(),
        icon_area.bottom() + 14,
        "ФАЙЛ НЕ ВЫБРАН",
        theme::TEXT,
        metrics.title,
    );
    font.draw_centered(
        canvas,
        area.center_x(),
        icon_area.bottom() + 18 + Font::glyph_height(metrics.title) + 6,
        "PNG / JPG / WEBP / BMP / GIF",
        theme::MUTED,
        metrics.small,
    );

    let label = "ОТКРЫТЬ ФАЙЛ";
    let button_w = (font.width(label, metrics.body) + 36).max(190);
    let button = Rect::new(
        area.center_x() - button_w / 2,
        icon_area.bottom() + 18 + Font::glyph_height(metrics.title) + 34,
        button_w,
        BUTTON_H + 4,
    );
    let mut action = Action::None;
    if widgets::button(canvas, font, button, label, true, ui, metrics.body) {
        action = Action::Open;
    }
    if focused(model, FocusId::Open) {
        widgets::focus_ring(canvas, button);
    }

    // Recent files: newest first, only in the empty state so the editor keeps
    // its full height for the preview.
    if !model.recent.is_empty() {
        let list_w = 470.min(area.w - 24).max(200);
        let x = area.center_x() - list_w / 2;
        let mut y = button.bottom() + 26;
        font.draw_centered(
            canvas,
            area.center_x(),
            y,
            "НЕДАВНИЕ ФАЙЛЫ",
            theme::MUTED,
            metrics.small,
        );
        y += Font::line_height(metrics.small) + 8;
        let limit = if area.h >= 520 { RECENT_VISIBLE } else { 3 };
        for (index, path) in model.recent.iter().take(limit).enumerate() {
            let row = Rect::new(x, y, list_w, 30);
            if row.bottom() > area.bottom() {
                break;
            }
            let hovered = ui.hovered(row);
            if hovered {
                canvas.fill_rect(row.x, row.y, row.w, row.h, theme::PANEL_HOVER);
            }
            canvas.hline(row.x, row.bottom(), row.w, theme::BORDER);
            let name = elide_middle(font, &path_name(path), list_w - 20, metrics.body);
            font.draw(
                canvas,
                row.x + 10,
                row.y + 3,
                &name,
                theme::TEXT,
                metrics.body,
            );
            let folder = path
                .parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_default();
            let folder = elide_middle(font, &folder, list_w - 20, metrics.small);
            font.draw(
                canvas,
                row.x + 10,
                row.y + 19,
                &folder,
                theme::MUTED,
                metrics.small,
            );
            if focused(model, FocusId::Recent(index)) {
                widgets::focus_ring(canvas, row);
            }
            if ui.clicked(row) && action == Action::None {
                action = Action::OpenRecent(index);
            }
            y += 32;
        }
    }

    action
}

/// Toast cards stacked above the status bar, newest closest to the bottom.
fn draw_toasts(canvas: &mut Canvas, model: &Model, font: &Font, metrics: Metrics, bottom: i32) {
    if model.toasts.is_empty() || canvas.width() < 200 {
        return;
    }
    let max_w = 560.min(canvas.width() - 40).max(180);
    let line_h = Font::line_height(metrics.small);
    let mut y = bottom;
    for toast in model.toasts.iter().rev() {
        let lines = wrap_limit(font, &toast.text, max_w - 20, metrics.small, 5);
        if lines.is_empty() {
            continue;
        }
        let text_w = lines
            .iter()
            .map(|line| font.width(line, metrics.small))
            .max()
            .unwrap_or(0);
        let w = (text_w + 20).min(max_w);
        let h = lines.len() as i32 * line_h + 10;
        y -= h;
        if y < OUTER_MARGIN + HEADER_H {
            break;
        }
        let x = (canvas.width() - w) / 2;
        let tone = match toast.tone {
            ToastTone::Info => UiToastTone::Info,
            ToastTone::Error => UiToastTone::Error,
        };
        widgets::toast_card(canvas, font, Rect::new(x, y, w, h), &lines, tone, metrics);
        y -= 8;
    }
}

/// Drag&drop hint drawn over the preview while a file is dragged into the
/// window. The drop itself is handled by the event loop.
fn draw_drop_overlay(
    canvas: &mut Canvas,
    font: &Font,
    metrics: Metrics,
    area: Rect,
    path: &std::path::Path,
) {
    if area.is_empty() {
        return;
    }
    canvas.set_clip(area);
    canvas.dither_rect(area.x, area.y, area.w, area.h, theme::BLUE, 0);
    let w = (area.w - 40).clamp(180, 420);
    let h = 92;
    let panel = Rect::new(area.center_x() - w / 2, area.center_y() - h / 2, w, h);
    canvas.fill_rect(panel.x + 2, panel.y + 2, panel.w, panel.h, theme::BLACK);
    canvas.fill_rect(panel.x, panel.y, panel.w, panel.h, theme::WHITE);
    canvas.stroke_rect(panel.x, panel.y, panel.w, panel.h, theme::BLUE);
    font.draw_centered(
        canvas,
        panel.center_x(),
        panel.y + 14,
        "ОТПУСТИТЕ ФАЙЛ",
        theme::TEXT,
        metrics.body,
    );
    let name = path_name(path);
    let name = elide_middle(font, &name, w - 24, metrics.small);
    font.draw_centered(
        canvas,
        panel.center_x(),
        panel.y + 40,
        &name,
        theme::MUTED,
        metrics.small,
    );
    font.draw_centered(
        canvas,
        panel.center_x(),
        panel.y + 60,
        "ИЗОБРАЖЕНИЕ ЗАГРУЗИТСЯ В ОКНО",
        theme::MUTED,
        metrics.small,
    );
    canvas.clear_clip();
}

fn draw_busy(
    canvas: &mut Canvas,
    ui: &UiState,
    font: &Font,
    metrics: Metrics,
    area: Rect,
    label: &str,
) {
    if area.is_empty() {
        return;
    }
    canvas.dither_rect(
        area.x,
        area.y,
        area.w,
        area.h,
        theme::BLACK,
        (ui.time * 8.0) as i32,
    );
    let text = format!("{label}{}", ".".repeat(((ui.time * 3.0) as usize) % 4));
    let width = 280.min(area.w - 20).max(180);
    let panel = Rect::new(area.center_x() - width / 2, area.center_y() - 30, width, 60);
    canvas.fill_rect(panel.x + 2, panel.y + 2, panel.w, panel.h, theme::BLACK);
    canvas.fill_rect(panel.x, panel.y, panel.w, panel.h, theme::WHITE);
    canvas.stroke_rect(panel.x, panel.y, panel.w, panel.h, theme::BLUE);
    font.draw_centered(
        canvas,
        panel.center_x(),
        panel.center_y() - Font::glyph_height(metrics.body) / 2,
        &text,
        theme::TEXT,
        metrics.body,
    );
}

fn draw_filter_menu(
    canvas: &mut Canvas,
    model: &Model,
    ui: &mut UiState,
    font: &Font,
    metrics: Metrics,
    anchor: Rect,
    bounds: Rect,
) -> Action {
    let values = filter_values();
    let width = anchor.w.max(150);
    let height = values.len() as i32 * theme::MENU_ITEM_H + 2;
    let x = anchor.x.min(bounds.right() - width - 4).max(bounds.x + 4);
    let below = anchor.bottom() + 2;
    let y = if below + height <= bounds.bottom() {
        below
    } else {
        anchor.y - height - 2
    };
    let panel = Rect::new(x, y, width, height);
    widgets::dropdown_panel(canvas, panel);
    let mut action = Action::None;
    for (index, filter) in values.iter().enumerate() {
        let item = Rect::new(
            panel.x + 1,
            panel.y + 1 + index as i32 * theme::MENU_ITEM_H,
            panel.w - 2,
            theme::MENU_ITEM_H,
        );
        if widgets::menu_item(
            canvas,
            font,
            item,
            filter_label(*filter),
            model.params.filter == *filter,
            ui,
            metrics.body,
        ) {
            action = Action::ChooseFilter(*filter);
        }
    }
    action
}

fn draw_status_bar(canvas: &mut Canvas, model: &Model, font: &Font, metrics: Metrics, rect: Rect) {
    let left = if model.is_saving() {
        match model.saving_target() {
            Some(SaveTarget::Export) => "ЭКСПОРТ...".to_string(),
            _ => "СОХРАНЕНИЕ...".to_string(),
        }
    } else if model.is_generating() {
        "ГЕНЕРАЦИЯ...".to_string()
    } else if model.is_cutting() {
        "ВЫРЕЗКА...".to_string()
    } else if model.loading.is_some() {
        "ЗАГРУЗКА...".to_string()
    } else {
        match model.status {
            Status::Empty => "ФАЙЛ НЕ ВЫБРАН".to_string(),
            Status::Loading => "ЗАГРУЗКА...".to_string(),
            Status::Ready
                if model.mode == Mode::Cutout
                    && model
                        .cutout_result
                        .as_ref()
                        .is_some_and(|result| result.fully_transparent) =>
            {
                "ГОТОВО | РЕЗУЛЬТАТ ПРОЗРАЧНЫЙ".to_string()
            }
            Status::Ready if model.is_dirty() => "ГОТОВО | НАСТРОЙКИ ИЗМЕНЕНЫ".to_string(),
            Status::Ready => "ГОТОВО".to_string(),
        }
    };
    let right = match model.mode {
        Mode::PixelArt => match &model.pixel_result {
            Some(result) => {
                let (out_w, out_h) = result.output_dims();
                format!(
                    "{} X {} | {} ЦВЕТОВ | {} МС",
                    out_w,
                    out_h,
                    result.stats.palette_colors,
                    result.elapsed.as_millis()
                )
            }
            None => source_label(model),
        },
        Mode::Cutout => match &model.cutout_result {
            Some(result) => {
                let removed = result.stats.background.as_ref().map_or_else(
                    || "ФОН: -".to_string(),
                    |background| format!("УДАЛЕНО {:.1}%", background.removed_percent),
                );
                format!(
                    "{} X {} | {} | {} МС",
                    result.width,
                    result.height,
                    removed,
                    result.elapsed.as_millis()
                )
            }
            None => source_label(model),
        },
    };
    widgets::status_bar(canvas, font, rect, &left, &right, metrics.body);
}

fn source_label(model: &Model) -> String {
    match &model.file {
        Some(file) => format!(
            "{} X {} / {}",
            file.meta.width, file.meta.height, file.meta.format
        ),
        None => format!("PIXEL-IMAGE {}", env!("CARGO_PKG_VERSION")),
    }
}

fn to_rgba(color: CoreColor) -> Color {
    [color[0], color[1], color[2], 255]
}

/// Tooltip text of a numeric or text field.
fn field_tip(field: Field) -> &'static str {
    match field {
        Field::Width => "ШИРИНА СЕТКИ, 1..512; СТРЕЛКИ ↑/↓ МЕНЯЮТ ШАГ",
        Field::Colors => "РАЗМЕР ПАЛИТРЫ, 2..256 ЦВЕТОВ",
        Field::Cell => "РАЗМЕР ОДНОЙ ЯЧЕЙКИ, 2..64 PX",
        Field::GridOpacity => "НЕПРОЗРАЧНОСТЬ ЛИНИЙ СЕТКИ, 0..100%",
        Field::GridColor => "ЦВЕТ ЛИНИЙ СЕТКИ, RRGGBB",
        Field::AlphaThreshold => "ПОРОГ ПРОЗРАЧНОСТИ, 1..255",
        Field::FillColor => "ЦВЕТ ЗАЛИВКИ ПРОЗРАЧНЫХ ЯЧЕЕК, RRGGBB",
        Field::Despeckle => "УДАЛЯТЬ ПЯТНА МЕНЬШЕ N ЯЧЕЕК; 0 — ВЫКЛЮЧЕНО",
        Field::BgColor => "ЦВЕТ ФОНА ДЛЯ РУЧНОГО РЕЖИМА, RRGGBB",
        Field::BgTolerance => "ДОПУСК ПОХОЖЕСТИ НА ФОН, 0..255",
        Field::BgGrow => "СКОЛЬКО СЛОЁВ КАЙМЫ СНЯТЬ, 0..16",
        Field::BgDefringe => "ШИРИНА МАТИРОВАНИЯ КРАЯ, 0..64",
        Field::HoleArea => "МИНИМАЛЬНАЯ ПЛОЩАДЬ ПРОСВЕТА, %",
        Field::OutputPrefix => "ИМЯ ФАЙЛОВ БЕЗ _GRID/_SPRITE; ПО УМОЛЧАНИЮ — ИМЯ ИСХОДНИКА",
    }
}

fn toggle_tip(toggle: Toggle) -> &'static str {
    match toggle {
        Toggle::Fill => "ЗАЛИВАТЬ ПРОЗРАЧНЫЕ ЯЧЕЙКИ ВЫБРАННЫМ ЦВЕТОМ",
        Toggle::RemoveBg => "УДАЛЯТЬ ОДНОТОННЫЙ ФОН ПЕРЕД ПИКСЕЛИЗАЦИЕЙ",
        Toggle::BgAutoColor => "ОПРЕДЕЛЯТЬ ЦВЕТ ФОНА ПО ПЕРИМЕТРУ",
        Toggle::BgAutoDefringe => "ПОДБИРАТЬ ШИРИНУ МАТИРОВАНИЯ ПО РАЗМЕРУ",
        Toggle::ThinGaps => "УДАЛЯТЬ ТОНКИЕ ЩЕЛИ ФОНА У ГРАНИЦЫ",
        Toggle::RemoveHoles => "УДАЛЯТЬ ЗАМКНУТЫЕ ПРОСВЕТЫ ФОНА",
    }
}

fn path_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn human_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes < KB {
        format!("{bytes} Б")
    } else if bytes < MB {
        format!("{:.1} КБ", bytes as f64 / KB as f64)
    } else if bytes < GB {
        format!("{:.1} МБ", bytes as f64 / MB as f64)
    } else {
        format!("{:.2} ГБ", bytes as f64 / GB as f64)
    }
}

/// Shorten a long path in the middle so the file name stays readable.
fn elide_middle(font: &Font, text: &str, max_width: i32, scale: i32) -> String {
    if font.width(text, scale) <= max_width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let adv = font.advance(scale).max(1);
    let max_chars = (max_width / adv).max(6) as usize;
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let left = keep / 2;
    let right = keep - left;
    let mut out = String::new();
    out.extend(chars.iter().take(left));
    out.push_str("...");
    out.extend(chars.iter().skip(chars.len() - right));
    out
}

fn wrap_limit(
    font: &Font,
    text: &str,
    max_width: i32,
    scale: i32,
    max_lines: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut width = 0;
        for ch in paragraph.chars() {
            let char_width = font.advance(scale);
            if width + char_width > max_width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                width = 0;
                if lines.len() >= max_lines {
                    return lines;
                }
            }
            line.push(ch);
            width += char_width;
        }
        lines.push(line);
        if lines.len() >= max_lines {
            return lines;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Прежний `elide_middle` (без заимствования): эталон для будущих
    /// оптимизаций этих функций.
    fn elide_middle_reference(font: &Font, text: &str, max_width: i32, scale: i32) -> String {
        if font.width(text, scale) <= max_width {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let adv = font.advance(scale).max(1);
        let max_chars = (max_width / adv).max(6) as usize;
        if chars.len() <= max_chars {
            return text.to_string();
        }
        let keep = max_chars.saturating_sub(3);
        let left = keep / 2;
        let right = keep - left;
        let mut out = String::new();
        out.extend(chars.iter().take(left));
        out.push_str("...");
        out.extend(chars.iter().skip(chars.len() - right));
        out
    }

    /// Усечение по центру и перенос по ширине обязаны совпадать с прежним
    /// выводом: это текст в UI, его нельзя менять «оптимизируя».
    #[test]
    fn elide_and_wrap_unchanged() {
        let font = Font::new();
        let scale = 2;
        let samples = [
            "",
            "a",
            "фото.jpg",
            "/очень/длинный/путь/к/файлу/с/картинкой.png",
            "C:\\Users\\пользователь\\Изображения\\отпуск-2026-финал.jpg",
            "кириллица",
            "1234567890",
            "ш",
            "СОХРАНЕНО",
            "上上下下左右左右",
        ];
        for text in samples {
            for max_width in [0, 7, 24, 64, 200, 4000] {
                assert_eq!(
                    elide_middle(&font, text, max_width, scale),
                    elide_middle_reference(&font, text, max_width, scale),
                    "усечение «{text}» при ширине {max_width}"
                );

                let wrapped = wrap_limit(&font, text, max_width.max(1), scale, 3);
                let reference: Vec<String> = wrap_limit_reference(&font, text, max_width.max(1), scale, 3);
                assert_eq!(wrapped, reference, "перенос «{text}» при ширине {max_width}");
            }
        }

        // Пустая строка, один символ и строка ровно в границу не усекаются.
        assert_eq!(elide_middle(&font, "", 100, scale), "");
        assert_eq!(elide_middle(&font, "ф", 100, scale), "ф");
        let exact = font.width("фото", scale);
        assert_eq!(elide_middle(&font, "фото", exact, scale), "фото");

        // Многострочный текст переносится по абзацам и уважает лимит строк.
        let lines = wrap_limit(&font, "один\nдва\nтри\nчетыре", 4000, scale, 3);
        assert_eq!(lines, vec!["один", "два", "три"]);
    }

    /// Эталон переноса — буквальная копия текущей реализации.
    fn wrap_limit_reference(
        font: &Font,
        text: &str,
        max_width: i32,
        scale: i32,
        max_lines: usize,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.split('\n') {
            let mut line = String::new();
            let mut width = 0;
            for ch in paragraph.chars() {
                let char_width = font.advance(scale);
                if width + char_width > max_width && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    width = 0;
                    if lines.len() >= max_lines {
                        return lines;
                    }
                }
                line.push(ch);
                width += char_width;
            }
            lines.push(line);
            if lines.len() >= max_lines {
                return lines;
            }
        }
        lines
    }
}
