//! winit application handler: window, GPU surface, input and redraws.

use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

use anyhow::Result;
use pixel_image_ui::{Canvas, Font, Metrics, Point, Rect, UiState, icons, theme};
use pixels::{Pixels, ScalingMode, SurfaceTexture};
use rfd::{AsyncFileDialog, MessageButtons, MessageDialog, MessageLevel};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey},
    monitor::MonitorHandle,
    window::{Icon, Window, WindowId},
};

use crate::{
    model::{
        ConfirmAction, Field, FocusId, Mode, Model, SaveRequest, SaveTarget, ToastTone, ZoomMode,
    },
    settings::{self, Settings},
    view::{self, Action},
    worker::{UserEvent, Worker, WorkerResult},
};

const INITIAL_W: f64 = 1360.0;
const INITIAL_H: f64 = 820.0;
const MIN_W: f64 = 1120.0;
const MIN_H: f64 = 700.0;
/// Space kept free around a restored window for the title bar, borders and
/// taskbar. winit exposes the monitor rectangle but not the work area.
const RESTORE_MARGIN_W: f64 = 32.0;
const RESTORE_MARGIN_H: f64 = 80.0;
const WHEEL_STEP: i32 = 30;
/// Idle time after the last change before the settings file is rewritten.
const SETTINGS_SAVE_DELAY: Duration = Duration::from_millis(500);

/// A pan drag in progress inside the preview.
struct PanDrag {
    origin: Point,
    start_pan: Point,
    moved: bool,
}

/// Result of an asynchronous native dialog.
enum DialogOutcome {
    Open(Option<PathBuf>),
    OutputDir(Option<PathBuf>),
}

/// One native dialog in flight, polled from the winit event loop.
type DialogFuture = Pin<Box<dyn Future<Output = DialogOutcome>>>;

/// Waker that turns dialog progress into a `UserEvent::Dialog` wake-up.
struct DialogWake(EventLoopProxy<UserEvent>);

impl Wake for DialogWake {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send_event(UserEvent::Dialog);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let _ = self.0.send_event(UserEvent::Dialog);
    }
}

pub struct App {
    window: Option<Arc<Window>>,
    pixels: Option<Pixels<'static>>,
    worker: Worker,
    /// Clone used to wake the loop when an async dialog makes progress.
    proxy: EventLoopProxy<UserEvent>,
    /// Native file/folder dialog currently in flight.
    dialog: Option<DialogFuture>,
    model: Model,
    ui: UiState,
    font: Font,
    metrics: Metrics,
    ui_scale: u32,
    surface: winit::dpi::PhysicalSize<u32>,
    pending_resize: Option<winit::dpi::PhysicalSize<u32>>,
    modifiers: ModifiersState,
    started: Instant,
    scroll_max: i32,
    sidebar: Rect,
    /// Preview body of the last frame: wheel zoom and pan hit-testing.
    preview: Rect,
    pan_limit: Point,
    fit_percent: Option<u32>,
    drag: Option<PanDrag>,
    /// Settings document location; `None` when the platform has no config dir.
    settings_path: Option<PathBuf>,
    saved_settings: Settings,
    /// Отпечаток состояния, соответствующего `saved_settings`.
    saved_signature: u64,
    save_pending: Option<Instant>,
    /// Logical window size persisted in the settings file. This is always the
    /// floating size: the client area of a maximized window follows the
    /// monitor and is not stored.
    window_size: Option<(u32, u32)>,
    /// Whether the window is currently maximized (persisted so the next start
    /// re-creates it maximized instead of restoring an oversized floating
    /// window).
    window_maximized: bool,
    /// Problems found while loading settings, shown as toasts once the window
    /// exists.
    pending_toasts: Vec<String>,
    /// Set after "ДОЖДАТЬСЯ И ВЫЙТИ"; the loop exits once the worker is idle.
    exit_requested: bool,
}

impl App {
    pub fn new(initial: Option<PathBuf>, proxy: EventLoopProxy<UserEvent>) -> Self {
        let settings_path = settings::settings_path();
        let loaded = settings::load(settings_path.as_deref());
        let mut model = Model::new();
        loaded.settings.apply(&mut model);
        // The restored settings are the starting point, not an undoable edit.
        model.reset_history();
        let window_size = loaded.settings.window_size();
        let window_maximized = loaded.settings.window.maximized;
        let saved_signature = Settings::signature(&model, window_size, window_maximized);
        let worker = Worker::spawn(proxy.clone());
        let mut app = Self {
            window: None,
            pixels: None,
            worker,
            proxy,
            dialog: None,
            model,
            ui: UiState::default(),
            font: Font::new(),
            metrics: Metrics::for_scale(1),
            ui_scale: 1,
            surface: winit::dpi::PhysicalSize::new(1, 1),
            pending_resize: None,
            modifiers: ModifiersState::empty(),
            started: Instant::now(),
            scroll_max: 0,
            sidebar: Rect::default(),
            preview: Rect::default(),
            pan_limit: Point::new(0, 0),
            fit_percent: None,
            drag: None,
            settings_path,
            saved_signature,
            saved_settings: loaded.settings,
            save_pending: None,
            window_size,
            window_maximized,
            pending_toasts: loaded.problem.into_iter().collect(),
            exit_requested: false,
        };
        if let Some(path) = initial {
            app.start_load(path);
        }
        app
    }

    fn start_load(&mut self, path: PathBuf) {
        let generation = self.model.begin_load(path.clone());
        self.worker.load(path, generation);
        self.request_redraw();
    }

    fn clear_file(&mut self) {
        self.model.clear();
        self.update_title();
        self.request_redraw();
    }

    fn update_title(&self) {
        let Some(window) = &self.window else {
            return;
        };
        match &self.model.file {
            Some(file) => {
                let marker = if self.model.is_dirty() { " *" } else { "" };
                window.set_title(&format!("PIXEL-IMAGE — {}{marker}", file.file_name()))
            }
            None => window.set_title("PIXEL-IMAGE"),
        }
    }

    fn open_dialog(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let mut dialog = AsyncFileDialog::new()
            .set_title("ОТКРЫТЬ ИЗОБРАЖЕНИЕ")
            .add_filter("Изображения", &["png", "jpg", "jpeg", "webp", "bmp", "gif"]);
        if let Some(directory) = self.dialog_directory() {
            dialog = dialog.set_directory(directory);
        }
        if let Some(window) = &self.window {
            dialog = dialog.set_parent(window.as_ref());
        }
        let future: DialogFuture = Box::pin(async move {
            let handle = dialog.pick_file().await;
            DialogOutcome::Open(handle.map(|handle| handle.path().to_path_buf()))
        });
        self.dialog = Some(future);
        // Poll once now so the dialog opens without waiting for another event;
        // further polls are driven by the waker and `about_to_wait`.
        self.poll_dialog();
        self.request_redraw();
    }

    /// Start folder of the file dialog: the current file, then the newest
    /// recent file, then the platform default.
    fn dialog_directory(&self) -> Option<PathBuf> {
        let source = self
            .model
            .file
            .as_ref()
            .map(|file| file.path.clone())
            .or_else(|| self.model.recent.first().cloned());
        source.and_then(|path| {
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(std::path::Path::to_path_buf)
        })
    }

    fn open_recent(&mut self, index: usize) {
        if let Some(path) = self.model.recent.get(index).cloned() {
            self.start_load(path);
        }
    }

    fn pick_output_dir(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let mut dialog = AsyncFileDialog::new().set_title("ПАПКА ДЛЯ ЭКСПОРТА");
        let current = self
            .model
            .output
            .dir
            .clone()
            .or_else(|| self.dialog_directory());
        if let Some(directory) = current {
            dialog = dialog.set_directory(directory);
        }
        if let Some(window) = &self.window {
            dialog = dialog.set_parent(window.as_ref());
        }
        let future: DialogFuture = Box::pin(async move {
            let handle = dialog.pick_folder().await;
            DialogOutcome::OutputDir(handle.map(|handle| handle.path().to_path_buf()))
        });
        self.dialog = Some(future);
        self.poll_dialog();
        self.request_redraw();
    }

    /// Poll the native dialog once. Ready futures are cleared and applied.
    fn poll_dialog(&mut self) {
        let outcome = {
            let Some(future) = self.dialog.as_mut() else {
                return;
            };
            let waker = Waker::from(Arc::new(DialogWake(self.proxy.clone())));
            let mut context = Context::from_waker(&waker);
            match future.as_mut().poll(&mut context) {
                Poll::Ready(outcome) => Some(outcome),
                Poll::Pending => None,
            }
        };
        if let Some(outcome) = outcome {
            self.dialog = None;
            self.apply_dialog(outcome);
        }
    }

    fn apply_dialog(&mut self, outcome: DialogOutcome) {
        match outcome {
            DialogOutcome::Open(Some(path)) => self.start_load(path),
            DialogOutcome::Open(None) => {}
            DialogOutcome::OutputDir(Some(dir)) => {
                // Only apply while the export dialog that opened the picker is
                // still up; Esc in the dialog drops the whole transaction.
                if self.model.export_dialog_open() {
                    self.model.set_output_dir(dir);
                    self.model.commit_history();
                    // Commit the typed prefix and let Enter start the export.
                    self.model.commit_edit();
                    self.model.focus = Some(FocusId::ExportSave);
                }
            }
            DialogOutcome::OutputDir(None) => {}
        }
        self.update_title();
        self.request_redraw();
    }

    /// "ВЗЯТЬ ЦВЕТ С УГЛА": sample the source pixel (0,0) as the manual
    /// background colour and report the result in a toast.
    fn pick_corner_color(&mut self) {
        self.model.commit_edit();
        self.model.focus = Some(FocusId::PickCornerColor);
        if let Some(color) = self.model.pick_corner_color() {
            self.model.toast(
                ToastTone::Info,
                format!(
                    "ЦВЕТ С УГЛА: #{:02X}{:02X}{:02X}",
                    color[0], color[1], color[2]
                ),
            );
        }
        self.update_title();
        self.request_redraw();
    }

    /// Undo the last parameter/output change.
    ///
    /// A field that is still being edited is committed first, so Ctrl+Z while
    /// typing reverts the typed value instead of silently dropping it.
    fn undo(&mut self) {
        self.model.commit_edit();
        self.model.commit_history();
        if self.model.undo() {
            self.update_title();
        }
        self.request_redraw();
    }

    /// Re-apply the change removed by the last undo.
    fn redo(&mut self) {
        self.model.commit_edit();
        self.model.commit_history();
        if self.model.redo() {
            self.update_title();
        }
        self.request_redraw();
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn init_window(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        if self.window.is_some() {
            return Ok(());
        }
        // The saved size is a hint: it is clamped to the application minimum
        // and to the monitor. Older documents stored the client area of a
        // maximized window as the floating size; such a size no longer fits
        // the screen once the non-client frame is added, so it is opened
        // maximized and clamped instead of partly off-screen.
        let bounds = restore_bounds(event_loop.primary_monitor().as_ref());
        let ((width, height), start_maximized, stored_size) =
            restored_window(self.window_size, self.window_maximized, bounds);
        self.window_size = stored_size;
        self.window_maximized = start_maximized;

        let icon = Icon::from_rgba(
            icons::app_icon_rgba(),
            icons::APP_ICON_SIZE,
            icons::APP_ICON_SIZE,
        )
        .ok();
        let attributes = Window::default_attributes()
            .with_title("PIXEL-IMAGE")
            .with_window_icon(icon)
            .with_inner_size(LogicalSize::new(width, height))
            .with_min_inner_size(LogicalSize::new(MIN_W, MIN_H))
            .with_maximized(start_maximized);
        // Windows maps the window immediately, but `Pixels`/wgpu still has to
        // initialize; the user would see an unpainted client area flash before
        // the first frame. Keep the window hidden until that frame is ready
        // (see `reveal_window`). `set_visible` is unsupported on Wayland, so
        // other platforms keep the previous behavior.
        #[cfg(windows)]
        let attributes = attributes.with_visible(false);
        let window = Arc::new(event_loop.create_window(attributes)?);
        let size = window.inner_size();
        self.ui_scale = ui_scale_for(&window);
        self.metrics = Metrics::for_scale(self.ui_scale);
        self.surface = size;
        // Remember the real floating size; the maximized client area must not
        // replace it, because that is what gets restored on the next start.
        self.window_maximized = window.is_maximized();
        if !self.window_maximized && window.fullscreen().is_none() {
            self.window_size = Some(logical_size(&window, size));
        }

        let surface =
            SurfaceTexture::new(size.width.max(1), size.height.max(1), Arc::clone(&window));
        let mut pixels = Pixels::new(size.width.max(1), size.height.max(1), surface)?;
        pixels.set_scaling_mode(ScalingMode::PixelPerfect);
        let clear = theme::DESKTOP;
        pixels.clear_color(pixels::wgpu::Color {
            r: f64::from(clear[0]) / 255.0,
            g: f64::from(clear[1]) / 255.0,
            b: f64::from(clear[2]) / 255.0,
            a: 1.0,
        });

        self.window = Some(Arc::clone(&window));
        self.pixels = Some(pixels);
        // Settings problems found before the window existed are shown now.
        for problem in std::mem::take(&mut self.pending_toasts) {
            self.model.toast(ToastTone::Error, problem);
        }
        self.reveal_window(&window);
        Ok(())
    }

    /// Show the window for the first time.
    ///
    /// On Windows the window is created hidden so the unpainted client area
    /// (black, with the old white rectangle) is never visible while wgpu is
    /// initializing. A hidden window gets no `RedrawRequested`, so the first
    /// frame is drawn directly and is already on screen when the window is
    /// revealed.
    fn reveal_window(&mut self, window: &Window) {
        // `set_visible` is unsupported on Wayland, so only Windows keeps the
        // window hidden until the first frame is ready.
        if cfg!(windows) {
            let _ = self.draw();
            window.set_visible(true);
        }
        window.request_redraw();
    }

    fn apply_pending_resize(&mut self) {
        let Some(size) = self.pending_resize.take() else {
            return;
        };
        if size.width == 0 || size.height == 0 {
            return;
        }
        let Some(pixels) = self.pixels.as_mut() else {
            return;
        };
        if let Err(error) = pixels.resize_surface(size.width, size.height) {
            crate::log_error!("resize surface error: {error}");
            return;
        }
        if let Err(error) = pixels.resize_buffer(size.width, size.height) {
            crate::log_error!("resize buffer error: {error}");
            return;
        }
        self.surface = size;
        self.ui_scale = self
            .window
            .as_ref()
            .map(|window| ui_scale_for(window))
            .unwrap_or(1);
        self.metrics = Metrics::for_scale(self.ui_scale);
        self.ui.pointer = None;
        // Store only the floating size for the settings document: while the
        // window is maximized (or fullscreen) its client area is dictated by
        // the monitor and is not a size to restore later.
        if let Some(window) = &self.window {
            self.window_maximized = window.is_maximized();
            if !self.window_maximized && window.fullscreen().is_none() {
                self.window_size = Some(logical_size(window, size));
            }
        }
    }

    fn handle_worker_results(&mut self) {
        let mut changed = false;
        while let Some(result) = self.worker.poll() {
            changed = true;
            match result {
                WorkerResult::Loaded {
                    generation,
                    path,
                    image,
                    meta,
                } => {
                    self.model.on_loaded(generation, path, image, meta);
                }
                WorkerResult::LoadFailed {
                    generation,
                    message,
                } => {
                    self.model.on_failed(generation, message);
                }
                WorkerResult::Generated {
                    job,
                    file,
                    sprite,
                    stats,
                    elapsed,
                } => {
                    self.model.on_generated(job, file, sprite, stats, elapsed);
                }
                WorkerResult::GenerateFailed { job, file, message } => {
                    self.model.on_generate_failed(job, file, message);
                }
                WorkerResult::Cutout {
                    job,
                    file,
                    display,
                    width,
                    height,
                    stats,
                    fully_transparent,
                    elapsed,
                } => {
                    self.model.on_cutout(
                        job,
                        file,
                        display,
                        width,
                        height,
                        stats,
                        fully_transparent,
                        elapsed,
                    );
                }
                WorkerResult::CutoutFailed { job, file, message } => {
                    self.model.on_cutout_failed(job, file, message);
                }
                WorkerResult::Saved {
                    job,
                    file,
                    grid_path,
                    sprite_path,
                    stats,
                    elapsed,
                } => {
                    self.model
                        .on_saved(job, file, grid_path, sprite_path, stats, elapsed);
                }
                WorkerResult::CutoutSaved {
                    job,
                    file,
                    path,
                    width,
                    height,
                    stats,
                    fully_transparent,
                    elapsed,
                } => {
                    self.model.on_cutout_saved(
                        job,
                        file,
                        path,
                        width,
                        height,
                        stats,
                        fully_transparent,
                        elapsed,
                    );
                }
                WorkerResult::SaveFailed { job, file, message } => {
                    self.model.on_save_failed(job, file, message);
                }
            }
        }
        if changed {
            self.update_title();
            self.request_redraw();
        }
    }

    /// Handle a key press. Returns true when the application should exit now
    /// (Ctrl+Q with an idle worker).
    fn handle_key(&mut self, event: &KeyEvent) -> bool {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let code = match event.physical_key {
            PhysicalKey::Code(code) => Some(code),
            PhysicalKey::Unidentified(_) => None,
        };

        if self.model.modal.is_some() {
            match event.logical_key {
                Key::Named(NamedKey::Escape) => self.cancel_modal(),
                Key::Named(NamedKey::Enter) => self.confirm_modal(),
                _ => {}
            }
            return false;
        }

        if self.model.export_dialog_open() {
            // The export dialog is modal: its four controls own the keyboard.
            // Ctrl shortcuts do not reach the editor while it is open.
            if ctrl {
                if code == Some(KeyCode::KeyA) && self.model.edit.is_some() {
                    self.model.select_all_edit();
                    self.request_redraw();
                }
                return false;
            }
            match event.logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.cancel_export();
                    return false;
                }
                Key::Named(NamedKey::Enter) => {
                    // Enter in the prefix field is the default action; on a
                    // button it activates that button ("ОБЗОР" included).
                    if self.model.edit.is_some() || self.model.focus.is_none() {
                        self.confirm_export();
                    } else {
                        self.activate_focus();
                    }
                    return false;
                }
                Key::Named(NamedKey::Tab) => {
                    self.focus_step(!shift);
                    return false;
                }
                _ => {}
            }
            if self.model.edit.is_some() {
                match &event.logical_key {
                    Key::Named(NamedKey::Backspace) => self.model.backspace(),
                    Key::Named(NamedKey::Delete) => self.model.delete_forward(),
                    Key::Named(NamedKey::ArrowLeft) => self.model.move_caret(-1),
                    Key::Named(NamedKey::ArrowRight) => self.model.move_caret(1),
                    Key::Named(NamedKey::Home) => self.model.caret_home(),
                    Key::Named(NamedKey::End) => self.model.caret_end(),
                    Key::Character(text) => {
                        for ch in text.chars() {
                            self.model.push_char(ch);
                        }
                    }
                    _ => {}
                }
                self.request_redraw();
                return false;
            }
            if event.logical_key == Key::Named(NamedKey::Space) {
                self.activate_focus();
            }
            return false;
        }

        if ctrl {
            match code {
                Some(KeyCode::KeyZ) if shift => {
                    self.redo();
                    return false;
                }
                Some(KeyCode::KeyZ) => {
                    self.undo();
                    return false;
                }
                Some(KeyCode::KeyY) => {
                    self.redo();
                    return false;
                }
                Some(KeyCode::KeyO) => {
                    self.open_dialog();
                    return false;
                }
                Some(KeyCode::KeyL) => {
                    self.clear_file();
                    return false;
                }
                Some(KeyCode::KeyG) => {
                    self.generate();
                    return false;
                }
                Some(KeyCode::KeyS) => {
                    self.save_clicked();
                    return false;
                }
                Some(KeyCode::KeyE) => {
                    self.export_clicked();
                    return false;
                }
                Some(KeyCode::KeyQ) => return self.request_quit(),
                Some(KeyCode::Minus) | Some(KeyCode::NumpadSubtract) => {
                    self.zoom_out();
                    return false;
                }
                Some(KeyCode::Equal) | Some(KeyCode::NumpadAdd) => {
                    self.zoom_in();
                    return false;
                }
                Some(KeyCode::Digit0) | Some(KeyCode::Numpad0) => {
                    self.zoom_fit();
                    return false;
                }
                Some(KeyCode::Digit1) | Some(KeyCode::Numpad1) => {
                    self.zoom_100();
                    return false;
                }
                Some(KeyCode::KeyA) if self.model.edit.is_some() => {
                    self.model.select_all_edit();
                    self.request_redraw();
                    return false;
                }
                _ => {}
            }
        }

        if event.logical_key == Key::Named(NamedKey::Escape) {
            if self.model.dropdown_open {
                self.model.dropdown_open = false;
            } else if self.model.edit.is_some() {
                self.model.cancel_edit();
            }
            self.model.focus = None;
            self.request_redraw();
            return false;
        }

        if self.model.dropdown_open {
            match event.logical_key {
                Key::Named(NamedKey::Enter) => {
                    self.model.dropdown_open = false;
                    self.request_redraw();
                }
                Key::Named(NamedKey::Tab) => {
                    self.model.dropdown_open = false;
                    self.focus_step(!shift);
                }
                _ => {}
            }
            return false;
        }

        if event.logical_key == Key::Named(NamedKey::Tab) {
            self.focus_step(!shift);
            return false;
        }

        if self.model.edit.is_some() {
            match &event.logical_key {
                Key::Named(NamedKey::Enter) => self.model.commit_edit(),
                Key::Named(NamedKey::Backspace) => self.model.backspace(),
                Key::Named(NamedKey::Delete) => self.model.delete_forward(),
                Key::Named(NamedKey::ArrowLeft) => self.model.move_caret(-1),
                Key::Named(NamedKey::ArrowRight) => self.model.move_caret(1),
                Key::Named(NamedKey::ArrowUp) => self.model.step_edit(1),
                Key::Named(NamedKey::ArrowDown) => self.model.step_edit(-1),
                Key::Named(NamedKey::Home) => self.model.caret_home(),
                Key::Named(NamedKey::End) => self.model.caret_end(),
                Key::Character(text) => {
                    for ch in text.chars() {
                        self.model.push_char(ch);
                    }
                }
                _ => {}
            }
            self.update_title();
            self.request_redraw();
            return false;
        }

        // Keyboard activation of the focused control ("доступность фокуса").
        if matches!(
            event.logical_key,
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space)
        ) {
            self.activate_focus();
        }
        false
    }

    /// Move the Tab focus to the next enabled control in visual order.
    fn focus_step(&mut self, forward: bool) {
        let order = view::focus_order(&self.model);
        if order.is_empty() {
            return;
        }
        let current = self
            .model
            .focus
            .and_then(|focus| order.iter().position(|candidate| *candidate == focus));
        let index = match current {
            Some(index) if forward => (index + 1) % order.len(),
            Some(index) => (index + order.len() - 1) % order.len(),
            None if forward => 0,
            None => order.len() - 1,
        };
        let focus = order[index];
        self.model.focus = Some(focus);
        if let FocusId::Field(field) = focus {
            // Text fields enter edit mode as before Stage 5.
            self.model.begin_edit(field);
        } else {
            self.model.commit_edit();
        }
        self.update_title();
        self.request_redraw();
    }

    /// Activate the control that owns keyboard focus (Enter / Space).
    fn activate_focus(&mut self) -> bool {
        let Some(focus) = self.model.focus else {
            return false;
        };
        match focus {
            FocusId::ModeTab(mode) => self.model.set_mode(mode),
            FocusId::Open => self.open_dialog(),
            FocusId::Clear => self.clear_file(),
            FocusId::Generate => self.generate(),
            FocusId::Save => self.save_clicked(),
            FocusId::Export => self.export_clicked(),
            FocusId::Filter => {
                if self.model.mode == Mode::PixelArt {
                    self.model.dropdown_open = true;
                }
            }
            FocusId::Field(field) => {
                self.model.commit_edit();
                self.model.begin_edit(field);
            }
            FocusId::Toggle(toggle) => self.model.toggle(toggle),
            FocusId::PickOutputDir => self.pick_output_dir(),
            FocusId::ExportSave => self.confirm_export(),
            FocusId::ExportCancel => self.cancel_export(),
            FocusId::PickCornerColor => self.pick_corner_color(),
            FocusId::Undo => self.undo(),
            FocusId::Redo => self.redo(),
            FocusId::ZoomIn => self.zoom_in(),
            FocusId::ZoomOut => self.zoom_out(),
            FocusId::ZoomFit => self.zoom_fit(),
            FocusId::Recent(index) => self.open_recent(index),
        }
        self.update_title();
        self.request_redraw();
        true
    }

    /// Close request / Ctrl+Q. Returns true when the app can exit immediately.
    fn request_quit(&mut self) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        if self.model.is_busy() {
            if !self.exit_requested {
                self.model.close_wait_modal();
                self.request_redraw();
            }
            return false;
        }
        true
    }

    fn zoom_in(&mut self) {
        self.model.zoom_in(self.fit_percent);
        self.request_redraw();
    }

    fn zoom_out(&mut self) {
        self.model.zoom_out(self.fit_percent);
        self.request_redraw();
    }

    fn zoom_fit(&mut self) {
        self.model.zoom_fit();
        self.request_redraw();
    }

    fn zoom_100(&mut self) {
        self.model.set_zoom(ZoomMode::Percent(100));
        self.request_redraw();
    }

    fn handle_action(&mut self, action: Action) {
        // Animation frames also pass through here; `None` must not request
        // another redraw or the event loop would spin.
        if action == Action::None {
            return;
        }
        if self.model.modal.is_some() {
            match action {
                Action::ConfirmModal => self.confirm_modal(),
                Action::CancelModal => self.cancel_modal(),
                _ => {}
            }
            return;
        }
        match action {
            Action::None => {}
            Action::Open => self.open_dialog(),
            Action::OpenRecent(index) => self.open_recent(index),
            Action::Clear => self.clear_file(),
            Action::SetMode(mode) => {
                self.model.set_mode(mode);
                self.model.focus = Some(FocusId::ModeTab(mode));
            }
            Action::Generate => self.generate(),
            Action::Save => self.save_clicked(),
            Action::Export => self.export_clicked(),
            Action::ConfirmExport => self.confirm_export(),
            Action::CancelExport => self.cancel_export(),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::PickOutputDir => self.pick_output_dir(),
            Action::PickCornerColor => self.pick_corner_color(),
            Action::ConfirmModal | Action::CancelModal => {}
            Action::BeginEdit(field) => {
                self.model.commit_edit();
                self.model.begin_edit(field);
                self.model.focus = Some(FocusId::Field(field));
            }
            Action::Step(field, delta) => {
                self.model.commit_edit();
                self.model.step_field(field, delta);
            }
            Action::Toggle(toggle) => {
                self.model.commit_edit();
                self.model.toggle(toggle);
                self.model.focus = Some(FocusId::Toggle(toggle));
            }
            Action::ToggleDropdown => {
                self.model.commit_edit();
                self.model.dropdown_open = !self.model.dropdown_open;
                self.model.focus = Some(FocusId::Filter);
            }
            Action::ChooseFilter(filter) => {
                self.model.apply_filter(filter);
                self.model.focus = Some(FocusId::Filter);
            }
            Action::SetScroll(offset) => {
                self.model.scroll = offset.clamp(0, self.scroll_max.max(0));
            }
            Action::ZoomIn => {
                self.model.focus = Some(FocusId::ZoomIn);
                self.zoom_in();
                return;
            }
            Action::ZoomOut => {
                self.model.focus = Some(FocusId::ZoomOut);
                self.zoom_out();
                return;
            }
            Action::ZoomFit => {
                self.model.focus = Some(FocusId::ZoomFit);
                self.zoom_fit();
                return;
            }
        }
        self.update_title();
        self.request_redraw();
    }

    fn confirm_modal(&mut self) {
        let confirm = self.model.modal.as_ref().map(|modal| modal.confirm);
        self.model.dismiss_modal();
        match confirm {
            Some(ConfirmAction::SaveOverwrite) => self.start_write(SaveTarget::Quick),
            Some(ConfirmAction::ExportOverwrite) => self.start_write(SaveTarget::Export),
            Some(ConfirmAction::SaveLarge) => self.prepare_write(SaveTarget::Quick, true),
            Some(ConfirmAction::ExportLarge) => self.prepare_write(SaveTarget::Export, true),
            Some(ConfirmAction::WaitAndExit) => self.exit_requested = true,
            Some(ConfirmAction::None) | None => {}
        }
        self.update_title();
        self.request_redraw();
    }

    fn cancel_modal(&mut self) {
        self.model.dismiss_modal();
        self.request_redraw();
    }

    fn generate(&mut self) {
        self.model.commit_edit();
        match self.model.mode {
            Mode::PixelArt => {
                if let Some(request) = self.model.begin_generate() {
                    self.worker
                        .generate(request.id, request.file, request.source, request.options);
                }
            }
            Mode::Cutout => {
                if let Some(request) = self.model.begin_cutout() {
                    self.worker.cutout(
                        request.id,
                        request.file,
                        request.source,
                        request.background,
                    );
                }
            }
        }
        self.update_title();
        self.request_redraw();
    }

    fn save_clicked(&mut self) {
        self.request_write(SaveTarget::Quick);
    }

    fn export_clicked(&mut self) {
        // Match the toolbar icon: the dialog only opens when a finished result
        // can actually be written.
        if !self.model.can_save() {
            return;
        }
        self.model.open_export_dialog();
        self.update_title();
        self.request_redraw();
    }

    /// "СОХРАНИТЬ" in the export dialog: validate the typed prefix, close the
    /// dialog and run the regular export preflight (limits, overwrite).
    fn confirm_export(&mut self) {
        if self.model.export_dialog_error().is_some() {
            // Keep the dialog open; the reason is drawn under the field.
            self.model.focus = Some(FocusId::Field(Field::OutputPrefix));
            self.model.begin_edit(Field::OutputPrefix);
            self.request_redraw();
            return;
        }
        self.model.commit_edit();
        self.model.finish_export_dialog();
        self.request_write(SaveTarget::Export);
    }

    /// "ОТМЕНА" / Esc in the export dialog: drop the folder/prefix changes.
    fn cancel_export(&mut self) {
        self.model.cancel_export_dialog();
        self.update_title();
        self.request_redraw();
    }

    fn request_write(&mut self, target: SaveTarget) {
        self.model.commit_edit();
        if !self.model.can_save() {
            return;
        }
        self.prepare_write(target, false);
        self.update_title();
        self.request_redraw();
    }

    /// Shared save/export preflight: prefix validation, size limits, warning
    /// and overwrite confirmation. `skip_warning` is used after the user has
    /// already accepted the size warning.
    fn prepare_write(&mut self, target: SaveTarget, skip_warning: bool) {
        if target == SaveTarget::Export
            && let Some(message) = self.model.export_prefix_error()
        {
            self.model.show_error("НЕВЕРНЫЙ ПРЕФИКС", message);
            return;
        }
        if let Some(message) = self.model.export_blocks() {
            self.model
                .show_error("СЛИШКОМ БОЛЬШОЕ ИЗОБРАЖЕНИЕ", message);
            return;
        }
        if !skip_warning && self.model.export_warning().is_some() {
            self.model.size_warning_modal(target);
            return;
        }
        let paths = match target {
            SaveTarget::Quick => self.model.save_paths(),
            SaveTarget::Export => self.model.export_paths(),
        };
        if paths.is_empty() {
            return;
        }
        if paths.iter().any(|path| path.exists()) {
            self.model.overwrite_modal(target);
        } else {
            self.start_write(target);
        }
    }

    fn start_write(&mut self, target: SaveTarget) {
        match self.model.begin_write(target) {
            Some(SaveRequest::Pixel {
                id,
                file,
                path,
                prefix,
                options,
            }) => {
                self.worker.save(id, file, path, prefix, options);
            }
            Some(SaveRequest::Cutout {
                id,
                file,
                path,
                prefix,
                background,
            }) => {
                self.worker.save_cutout(id, file, path, prefix, background);
            }
            None => {}
        }
        self.request_redraw();
    }

    fn scroll_by(&mut self, lines: i32) {
        let next = self.model.scroll - lines * WHEEL_STEP;
        self.model.scroll = next.clamp(0, self.scroll_max.max(0));
    }

    /// Read the live maximized state. `Resized` normally updates it, but an
    /// immediate close after maximizing may happen before the next frame.
    fn refresh_window_state(&mut self) {
        if let Some(window) = &self.window {
            self.window_maximized = window.is_maximized();
        }
    }

    /// Rewrite the settings document once the user has been idle for
    /// [`SETTINGS_SAVE_DELAY`]. Called from every redraw and from
    /// `about_to_wait`, so the deferred save always happens.
    fn maybe_save_settings(&mut self) {
        let Some(path) = self.settings_path.clone() else {
            return;
        };
        self.refresh_window_state();
        // Отпечаток вместо `Settings::from_model`: на статичном кадре это
        // сравнение двух `u64`, а не десяток `String` и список недавних файлов.
        let signature =
            Settings::signature(&self.model, self.window_size, self.window_maximized);
        if signature == self.saved_signature {
            self.save_pending = None;
            return;
        }
        let now = Instant::now();
        match self.save_pending {
            None => self.save_pending = Some(now + SETTINGS_SAVE_DELAY),
            Some(due) if now >= due => {
                self.save_pending = None;
                let current =
                    Settings::from_model(&self.model, self.window_size, self.window_maximized);
                match settings::save(&path, &current) {
                    Ok(()) => {
                        self.saved_settings = current;
                        self.saved_signature = signature;
                    }
                    Err(problem) => {
                        // Do not retry on a broken destination; report once.
                        self.saved_settings = current;
                        self.saved_signature = signature;
                        self.model.toast(ToastTone::Error, problem);
                        self.request_redraw();
                    }
                }
            }
            Some(_) => {}
        }
    }

    /// Write the settings unconditionally (application exit).
    fn save_settings_now(&mut self) {
        let Some(path) = self.settings_path.clone() else {
            return;
        };
        self.refresh_window_state();
        let current = Settings::from_model(&self.model, self.window_size, self.window_maximized);
        match settings::save(&path, &current) {
            Ok(()) => {
                self.saved_signature =
                    Settings::signature(&self.model, self.window_size, self.window_maximized);
                self.saved_settings = current;
                self.save_pending = None;
            }
            Err(problem) => crate::log_error!("settings save failed: {problem}"),
        }
    }

    fn draw(&mut self) -> Action {
        let Some(pixels) = self.pixels.as_mut() else {
            return Action::None;
        };
        let surface = self.surface;
        let scale = self.ui_scale;
        let frame = {
            let frame = pixels.frame_mut();
            let mut canvas = Canvas::new(frame, surface.width.max(1), surface.height.max(1), scale);
            view::draw(
                &mut canvas,
                &self.model,
                &mut self.ui,
                &self.font,
                self.metrics,
            )
        };
        if let Err(error) = pixels.render() {
            crate::log_error!("pixels render error: {error}");
        }
        self.scroll_max = frame.scroll_max;
        self.sidebar = frame.sidebar;
        self.preview = frame.preview;
        self.pan_limit = frame.pan_limit;
        self.fit_percent = frame.fit_percent;
        if self.model.zoom != ZoomMode::Fit {
            // The visible panes may be narrower than the stored offset after a
            // zoom or window resize; keep the model in range.
            self.model.pan = Point::new(
                self.model.pan.x.clamp(-self.pan_limit.x, self.pan_limit.x),
                self.model.pan.y.clamp(-self.pan_limit.y, self.pan_limit.y),
            );
        } else {
            self.model.pan = Point::new(0, 0);
        }
        let mut click_handled = false;
        if self.ui.click_pending && self.model.modal.is_none() {
            let popup = self.model.dropdown_open;
            let editing = self.model.edit.is_some();
            if popup {
                self.model.dropdown_open = false;
            }
            self.model.commit_edit();
            click_handled = popup || editing;
        }
        self.ui.end_frame();
        if click_handled {
            self.update_title();
            self.request_redraw();
        }
        frame.action
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.init_window(event_loop) {
            crate::log_error!("failed to initialize window: {error:#}");
            let _ = MessageDialog::new()
                .set_level(MessageLevel::Error)
                .set_title("PIXEL-IMAGE")
                .set_description(format!("НЕ УДАЛОСЬ СОЗДАТЬ ОКНО\n\n{error:#}"))
                .set_buttons(MessageButtons::Ok)
                .show();
            event_loop.exit();
            return;
        }
        // A file passed on the command line may already be decoded before the
        // event loop starts; draining here also covers a lost proxy wake-up.
        self.handle_worker_results();
        event_loop.set_control_flow(ControlFlow::Wait);
        self.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if window.id() != window_id {
            return;
        }

        let mut redraw = false;
        match event {
            WindowEvent::CloseRequested => {
                if self.dialog.is_some() {
                    // A native dialog is modal to this window; wait for it to
                    // be confirmed or cancelled before tearing the window down.
                    return;
                }
                if self.model.is_busy() {
                    // Never drop a running worker mid-write: ask whether to
                    // wait for it. A second close request is ignored while the
                    // exit is already pending.
                    if !self.exit_requested {
                        self.model.close_wait_modal();
                        self.request_redraw();
                    }
                    return;
                }
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                self.pending_resize = Some(size);
                redraw = true;
            }
            // Native drag&drop: the OS reports the hovered file before the
            // drop, so the window can show where it will go. The load itself
            // uses the same generation-guarded path as the file dialog.
            WindowEvent::HoveredFile(path) => {
                self.model.drop_hover = Some(path);
                redraw = true;
            }
            WindowEvent::HoveredFileCancelled => {
                self.model.drop_hover = None;
                redraw = true;
            }
            WindowEvent::DroppedFile(path) => {
                self.model.drop_hover = None;
                self.start_load(path);
                return;
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.pending_resize = Some(window.inner_size());
                redraw = true;
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.ui_scale.max(1) as i32;
                let point = Point::new(position.x as i32 / scale, position.y as i32 / scale);
                self.ui.pointer = Some(point);
                if let Some(drag) = self.drag.as_mut() {
                    let dx = point.x - drag.origin.x;
                    let dy = point.y - drag.origin.y;
                    if dx != 0 || dy != 0 {
                        drag.moved = true;
                    }
                    let pan = Point::new(
                        (drag.start_pan.x + dx).clamp(-self.pan_limit.x, self.pan_limit.x),
                        (drag.start_pan.y + dy).clamp(-self.pan_limit.y, self.pan_limit.y),
                    );
                    self.model.set_pan(pan);
                }
                redraw = true;
            }
            WindowEvent::CursorLeft { .. } => {
                self.ui.pointer = None;
                self.drag = None;
                redraw = true;
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => {
                    self.ui.mouse_down = true;
                    // Panning only makes sense at a fixed zoom and only inside
                    // the preview body.
                    if self.model.modal.is_none()
                        && !self.model.dropdown_open
                        && self.model.zoom != ZoomMode::Fit
                        && let Some(pointer) = self.ui.pointer
                        && self.preview.contains(pointer)
                    {
                        self.drag = Some(PanDrag {
                            origin: pointer,
                            start_pan: self.model.pan,
                            moved: false,
                        });
                    }
                    redraw = true;
                }
                ElementState::Released => {
                    self.ui.mouse_down = false;
                    let dragged = self.drag.take().is_some_and(|drag| drag.moved);
                    // A drag is not a click: do not activate anything under it.
                    self.ui.click_pending = !dragged;
                    redraw = true;
                }
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y.round() as i32,
                    MouseScrollDelta::PixelDelta(position) => {
                        (position.y / WHEEL_STEP as f64).round() as i32
                    }
                };
                if self.model.modal.is_none()
                    && !self.model.export_dialog_open()
                    && lines != 0
                    && let Some(point) = self.ui.pointer
                {
                    if self.sidebar.contains(point) {
                        // The settings list scrolls; a wheel step is one row.
                        self.model.dropdown_open = false;
                        self.scroll_by(lines);
                        redraw = true;
                    } else if self.preview.contains(point) {
                        // The preview zooms; negative wheel deltas zoom out.
                        for _ in 0..lines.abs() {
                            if lines > 0 {
                                self.model.zoom_in(self.fit_percent);
                            } else {
                                self.model.zoom_out(self.fit_percent);
                            }
                        }
                        redraw = true;
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let acted = event.state == ElementState::Pressed && !event.repeat;
                let exit = acted && self.handle_key(&event);
                if acted {
                    // Record a parameter change made by typing/arrow keys.
                    self.model.commit_history();
                }
                if exit {
                    // Ctrl+Q with an idle worker closes immediately.
                    event_loop.exit();
                    return;
                }
            }
            WindowEvent::Focused(false) => {
                self.ui.mouse_down = false;
                self.drag = None;
                self.ui.pointer = None;
            }
            WindowEvent::RedrawRequested => {
                self.apply_pending_resize();
                self.ui.time = self.started.elapsed().as_secs_f32();
                let action = self.draw();
                self.handle_action(action);
                // `draw` may also commit a pending field edit when the click
                // landed outside it; record whatever changed in this frame.
                self.model.commit_history();
                self.maybe_save_settings();
            }
            _ => {}
        }

        if redraw {
            window.request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Worker => self.handle_worker_results(),
            UserEvent::Dialog => self.poll_dialog(),
        }
        if self.exit_requested && !self.model.is_busy() {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The proxy is the normal wake-up path, but polling here makes early
        // worker results reliable even if a wake-up raced the loop startup.
        self.handle_worker_results();
        if self.dialog.is_some() {
            self.poll_dialog();
        }
        self.maybe_save_settings();
        if self.model.expire_toasts()
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        if self.exit_requested && !self.model.is_busy() {
            event_loop.exit();
            return;
        }

        // Wake up for the next toast expiry, the deferred settings write and
        // the 80 ms animation tick while a worker is busy.
        let mut wake = self.model.next_toast_deadline();
        if let Some(due) = self.save_pending {
            wake = Some(wake.map_or(due, |current| current.min(due)));
        }
        if self.model.is_busy() || self.model.edit.is_some() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            let tick = Instant::now() + Duration::from_millis(80);
            wake = Some(wake.map_or(tick, |current| current.min(tick)));
        }
        // Some native dialog futures only make progress when polled; keep a
        // slow tick while one is open. The waker still delivers the fast path.
        if self.dialog.is_some() {
            let tick = Instant::now() + Duration::from_millis(80);
            wake = Some(wake.map_or(tick, |current| current.min(tick)));
        }
        match wake {
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.save_settings_now();
    }
}

fn ui_scale_for(window: &Window) -> u32 {
    (window.scale_factor().round() as u32).max(1)
}

/// Logical client size, which is what the settings document stores.
fn logical_size(window: &Window, physical: winit::dpi::PhysicalSize<u32>) -> (u32, u32) {
    let logical = physical.to_logical::<f64>(window.scale_factor().max(1.0));
    (
        logical.width.round().max(1.0) as u32,
        logical.height.round().max(1.0) as u32,
    )
}

/// Largest client size that still leaves the title bar, borders and taskbar
/// reachable. winit exposes the monitor rectangle but not the work area, so
/// the margins are conservative.
fn restore_bounds(monitor: Option<&MonitorHandle>) -> Option<(f64, f64)> {
    let monitor = monitor?;
    let logical = monitor
        .size()
        .to_logical::<f64>(monitor.scale_factor().max(1.0));
    Some((
        (logical.width - RESTORE_MARGIN_W).max(MIN_W),
        (logical.height - RESTORE_MARGIN_H).max(MIN_H),
    ))
}

/// Decide the client size and maximized state for a window re-created from
/// settings, and the floating size to keep in the document.
///
/// An older build could store the client area of a maximized window as the
/// floating size. That size exceeds [`restore_bounds`] once decorations are
/// added, so the window is opened maximized and clamped instead of appearing
/// partly off-screen.
fn restored_window(
    saved: Option<(u32, u32)>,
    maximized: bool,
    bounds: Option<(f64, f64)>,
) -> ((f64, f64), bool, Option<(u32, u32)>) {
    let Some((width, height)) = saved else {
        return (
            (INITIAL_W, INITIAL_H),
            maximized,
            Some((INITIAL_W as u32, INITIAL_H as u32)),
        );
    };
    let mut width = f64::from(width).max(MIN_W);
    let mut height = f64::from(height).max(MIN_H);
    let mut start_maximized = maximized;
    if let Some((max_width, max_height)) = bounds
        && (width > max_width || height > max_height)
    {
        start_maximized = true;
        width = width.min(max_width);
        height = height.min(max_height);
    }
    (
        (width, height),
        start_maximized,
        Some((width.round() as u32, height.round() as u32)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_HD_BOUNDS: (f64, f64) = (1888.0, 1000.0);

    #[test]
    fn stale_maximized_size_restores_maximized_and_fits() {
        // Client area of a maximized 1920x1080 window saved as a floating size.
        let ((width, height), maximized, stored) =
            restored_window(Some((1920, 1040)), false, Some(FULL_HD_BOUNDS));

        assert!(maximized);
        assert_eq!((width, height), FULL_HD_BOUNDS);
        assert_eq!(stored, Some((1888, 1000)));
    }

    #[test]
    fn normal_size_restores_as_a_floating_window() {
        let ((width, height), maximized, stored) =
            restored_window(Some((1280, 800)), false, Some(FULL_HD_BOUNDS));

        assert!(!maximized);
        assert_eq!((width, height), (1280.0, 800.0));
        assert_eq!(stored, Some((1280, 800)));
    }

    #[test]
    fn maximized_flag_is_restored_with_the_floating_size() {
        let ((width, height), maximized, stored) =
            restored_window(Some((1280, 800)), true, Some(FULL_HD_BOUNDS));

        assert!(maximized);
        assert_eq!((width, height), (1280.0, 800.0));
        assert_eq!(stored, Some((1280, 800)));
    }

    #[test]
    fn missing_size_uses_the_initial_size() {
        let ((width, height), maximized, stored) = restored_window(None, true, None);

        assert!(maximized);
        assert_eq!((width, height), (INITIAL_W, INITIAL_H));
        assert_eq!(stored, Some((INITIAL_W as u32, INITIAL_H as u32)));
    }
}
