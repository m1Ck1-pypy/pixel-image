//! Application state shared by the event handler and the view.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use image::RgbaImage;
use pixel_image_core::{
    BackgroundOptions, CleanStats, Color, Filter, Options, Stats, color::parse_hex, output_path,
};
use pixel_image_ui::Point;

use crate::worker::FileMeta;

/// How many toasts may be on screen at once; older ones are dropped.
pub const TOAST_LIMIT: usize = 3;
/// Lifetime of one toast.
pub const TOAST_TTL: Duration = Duration::from_secs(4);
/// How many parameter changes can be undone.
pub const HISTORY_LIMIT: usize = 100;

/// Zoom levels offered by the preview toolbar, in percent of the original
/// pixel size (100% = one logical screen pixel per image pixel).
pub const ZOOM_STEPS: [u32; 7] = [25, 50, 100, 200, 400, 800, 1600];

/// Preview scale: fit the pane or a fixed percentage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomMode {
    Fit,
    Percent(u32),
}

impl ZoomMode {
    pub fn percent(self) -> Option<u32> {
        match self {
            ZoomMode::Fit => None,
            ZoomMode::Percent(percent) => Some(percent),
        }
    }

    /// Percentage shown in the toolbar; `fit` is the measured fit percentage
    /// so the "ПО РАЗМЕРУ" mode still tells the user what it resolved to.
    pub fn label(self, fit: Option<u32>) -> String {
        format!("{}%", self.percent().or(fit).unwrap_or(100))
    }

    /// Next discrete step up. From "по размеру" it steps up from the measured
    /// fit percentage, so a small image does not jump to 100%.
    pub fn next_in(self, fit: Option<u32>) -> Self {
        let current = self.percent().or(fit).unwrap_or(100);
        let next = ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > current)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        ZoomMode::Percent(next)
    }

    pub fn next_out(self, fit: Option<u32>) -> Self {
        let current = self.percent().or(fit).unwrap_or(100);
        let previous = ZOOM_STEPS
            .iter()
            .copied()
            .rev()
            .find(|step| *step < current)
            .unwrap_or(ZOOM_STEPS[0]);
        ZoomMode::Percent(previous)
    }
}

/// Keyboard focus target. The visual order is built by
/// [`crate::view::focus_order`], which walks the same rows the view draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusId {
    ModeTab(Mode),
    Open,
    Clear,
    Generate,
    Save,
    Export,
    /// Undo/redo chips in the sidebar header.
    Undo,
    Redo,
    /// "СБРОСИТЬ НАСТРОЙКИ" chip right after the sidebar caption.
    ResetSettings,
    Filter,
    Field(Field),
    Toggle(Toggle),
    /// "ОБЗОР" inside the export dialog.
    PickOutputDir,
    /// "СОХРАНИТЬ" and "ОТМЕНА" inside the export dialog.
    ExportSave,
    ExportCancel,
    /// "ВЗЯТЬ ЦВЕТ С УГЛА" in the background block.
    PickCornerColor,
    ZoomOut,
    ZoomIn,
    ZoomFit,
    /// Entry in the "НЕДАВНИЕ ФАЙЛЫ" list of the empty state.
    Recent(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastTone {
    Info,
    Error,
}

/// One notification in the toast queue.
#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub tone: ToastTone,
    pub expires: Instant,
}

pub struct LoadedFile {
    pub path: PathBuf,
    pub image: Arc<RgbaImage>,
    pub meta: FileMeta,
}

impl LoadedFile {
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Editor mode selected by the toolbar tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    PixelArt,
    Cutout,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::PixelArt, Mode::Cutout];

    pub fn label(self) -> &'static str {
        match self {
            Mode::PixelArt => "ПИКСЕЛЬ-АРТ",
            Mode::Cutout => "ВЫРЕЗАТЬ ФОН",
        }
    }
}

/// Background removal settings. Both modes use them: the pixel-art pipeline
/// only applies them when "удалить фон" is checked, the cutout mode always
/// does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackgroundParams {
    pub auto_color: bool,
    pub color: Color,
    pub tolerance: u8,
    pub grow: u32,
    pub auto_defringe: bool,
    pub defringe: u32,
    pub thin_gaps: bool,
    pub remove_holes: bool,
    pub hole_area_pct: f32,
}

impl BackgroundParams {
    pub fn options(self) -> BackgroundOptions {
        BackgroundOptions {
            color: (!self.auto_color).then_some(self.color),
            tolerance: self.tolerance,
            grow: self.grow,
            defringe: (!self.auto_defringe).then_some(self.defringe),
            thin_gaps: self.thin_gaps,
            remove_holes: self.remove_holes,
            hole_area_pct: self.hole_area_pct,
        }
    }
}

/// Where a completed save writes: next to the source (quick save) or to the
/// user-selected output folder/prefix (Stage 4 export).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveTarget {
    Quick,
    Export,
}

/// Output folder and prefix of the Stage 4 export.
///
/// `None` keeps the core default: the source folder and the source file stem.
/// Both values are resolved lazily so opening another file follows the new
/// name until the user overrides it. The values are edited in the export
/// dialog (see [`ExportDialog`]) and persisted with the rest of the settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputParams {
    pub dir: Option<PathBuf>,
    pub prefix: Option<String>,
}

/// Open "ЭКСПОРТ" dialog.
///
/// The dialog edits [`Model::output`] in place so the prefix field reuses the
/// regular edit machinery; `base` is what "ОТМЕНА" restores.
pub struct ExportDialog {
    base: OutputParams,
}

pub struct LoadJob {
    pub path: PathBuf,
}

pub struct GenerateJob {
    pub id: u64,
    /// File generation the job was started for; stale results are ignored.
    pub file: u64,
    pub params: PixelParams,
}

pub struct CutoutJob {
    pub id: u64,
    /// File generation the job was started for; stale results are ignored.
    pub file: u64,
    pub params: BackgroundParams,
}

pub struct SaveJob {
    pub id: u64,
    pub file: u64,
    pub kind: Mode,
    pub target: SaveTarget,
}

pub struct GenerateRequest {
    pub id: u64,
    pub file: u64,
    pub source: Arc<RgbaImage>,
    pub options: Options,
    pub params: PixelParams,
}

pub struct CutoutRequest {
    pub id: u64,
    pub file: u64,
    pub source: Arc<RgbaImage>,
    pub background: BackgroundOptions,
}

pub enum SaveRequest {
    Pixel {
        id: u64,
        file: u64,
        path: PathBuf,
        /// Absolute base path for export; `None` writes next to the source.
        prefix: Option<String>,
        options: Options,
    },
    Cutout {
        id: u64,
        file: u64,
        path: PathBuf,
        /// Absolute base path for export; `None` writes next to the source.
        prefix: Option<String>,
        background: BackgroundOptions,
    },
}

pub struct GeneratedImage {
    pub sprite: Arc<RgbaImage>,
    pub stats: Stats,
    pub elapsed: Duration,
    /// Parameter snapshot used for the conversion. The preview and the export
    /// both come from this snapshot.
    pub params: PixelParams,
}

impl GeneratedImage {
    /// Size of the exported grid image (`grid * cell`).
    pub fn output_dims(&self) -> (u64, u64) {
        (
            self.stats.grid_width as u64 * self.params.cell as u64,
            self.stats.grid_height as u64 * self.params.cell as u64,
        )
    }
}

/// Cutout preview result.
///
/// `display` is a downscaled copy for the screen (the worker caps the long
/// side at 4096 px); `width`/`height` are the dimensions of the real cutout and
/// therefore of the exported `_cut.png`.
pub struct CutoutImage {
    pub display: Arc<RgbaImage>,
    pub width: u32,
    pub height: u32,
    pub stats: CleanStats,
    pub fully_transparent: bool,
    pub elapsed: Duration,
    /// Background snapshot used for the cutout. The preview and the export both
    /// come from this snapshot.
    pub params: BackgroundParams,
}

impl CutoutImage {
    pub fn display_dims(&self) -> (u32, u32) {
        self.display.dimensions()
    }

    pub fn is_downscaled(&self) -> bool {
        self.display_dims() != (self.width, self.height)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalTone {
    Error,
    Info,
    Warning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalButtons {
    Ok,
    OkCancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    None,
    SaveOverwrite,
    ExportOverwrite,
    SaveLarge,
    ExportLarge,
    WaitAndExit,
}

pub struct Modal {
    pub title: String,
    pub message: String,
    pub tone: ModalTone,
    pub buttons: ModalButtons,
    pub confirm: ConfirmAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Empty,
    Loading,
    Ready,
}

/// Text-editable settings. Dropdowns and checkboxes are separate types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Width,
    Colors,
    Cell,
    GridOpacity,
    GridColor,
    AlphaThreshold,
    FillColor,
    Despeckle,
    BgColor,
    BgTolerance,
    BgGrow,
    BgDefringe,
    HoleArea,
    OutputPrefix,
}

/// Visual (and Tab) order of the text fields.
pub const FIELD_ORDER: [Field; 14] = [
    Field::Width,
    Field::Colors,
    Field::Cell,
    Field::GridOpacity,
    Field::GridColor,
    Field::AlphaThreshold,
    Field::FillColor,
    Field::Despeckle,
    Field::BgColor,
    Field::BgTolerance,
    Field::BgGrow,
    Field::BgDefringe,
    Field::HoleArea,
    Field::OutputPrefix,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    Fill,
    RemoveBg,
    BgAutoColor,
    BgAutoDefringe,
    ThinGaps,
    RemoveHoles,
}

/// Everything the CLI can configure for a pixel-art conversion, except the
/// cutout mode and the output prefix (kept separately in [`OutputParams`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelParams {
    pub width: u32,
    pub filter: Filter,
    pub colors: u16,
    pub cell: u32,
    pub grid_opacity: u8,
    pub grid_color: Color,
    pub alpha_threshold: u8,
    pub fill_enabled: bool,
    pub fill_color: Color,
    pub despeckle: u32,
    pub remove_bg: bool,
    pub bg_auto_color: bool,
    pub bg_color: Color,
    pub bg_tolerance: u8,
    pub bg_grow: u32,
    pub bg_auto_defringe: bool,
    pub bg_defringe: u32,
    pub thin_gaps: bool,
    pub remove_holes: bool,
    pub hole_area_pct: f32,
}

impl Default for PixelParams {
    fn default() -> Self {
        Self {
            width: 128,
            filter: Filter::Lanczos,
            colors: 64,
            cell: 16,
            grid_opacity: 27,
            grid_color: [0, 0, 0],
            alpha_threshold: 128,
            fill_enabled: false,
            fill_color: [255, 255, 255],
            despeckle: 2,
            remove_bg: false,
            bg_auto_color: true,
            bg_color: [255, 255, 255],
            bg_tolerance: 12,
            bg_grow: 0,
            bg_auto_defringe: true,
            bg_defringe: 0,
            thin_gaps: false,
            remove_holes: false,
            hole_area_pct: 0.04,
        }
    }
}

impl PixelParams {
    /// Background settings, independent of whether pixel-art mode applies them.
    pub fn background_params(&self) -> BackgroundParams {
        BackgroundParams {
            auto_color: self.bg_auto_color,
            color: self.bg_color,
            tolerance: self.bg_tolerance,
            grow: self.bg_grow,
            auto_defringe: self.bg_auto_defringe,
            defringe: self.bg_defringe,
            thin_gaps: self.thin_gaps,
            remove_holes: self.remove_holes,
            hole_area_pct: self.hole_area_pct,
        }
    }

    /// Parameters for the real conversion (the cell size used by export).
    pub fn options(&self) -> Options {
        let background_removal = self.remove_bg.then(|| self.background_params().options());
        Options {
            width: self.width,
            filter: self.filter,
            colors: self.colors,
            cell: self.cell,
            grid_opacity: self.grid_opacity,
            grid_color: self.grid_color,
            alpha_threshold: self.alpha_threshold,
            fill: self.fill_enabled.then_some(self.fill_color),
            despeckle: self.despeckle,
            background_removal,
        }
    }

    /// Preview conversion: the sprite at grid resolution, the grid is drawn by
    /// the UI. This avoids building a huge grid bitmap just to show it.
    pub fn preview_options(&self) -> Options {
        let mut options = self.options();
        options.cell = 1;
        options
    }

    pub fn error(&self) -> Option<String> {
        self.options()
            .validate()
            .err()
            .map(|error| error.to_string())
    }
}

/// User-editable settings captured for undo/redo.
///
/// File state, preview results and view state (zoom/pan/focus) are not part of
/// the snapshot: undo only walks the parameter and output history.
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    params: PixelParams,
    output: OutputParams,
}

#[derive(Clone, Debug)]
pub struct Edit {
    pub field: Field,
    pub text: String,
    pub caret: usize,
    pub select_all: bool,
}

impl Edit {
    pub fn invalid(&self) -> bool {
        parse_field(self.field, &self.text).is_none()
    }
}

pub struct Model {
    pub file: Option<LoadedFile>,
    pub loading: Option<LoadJob>,
    pub mode: Mode,
    pub generating: Option<GenerateJob>,
    pub cutting: Option<CutoutJob>,
    pub saving: Option<SaveJob>,
    pub modal: Option<Modal>,
    /// Open export dialog (folder, prefix and the settings to restore on
    /// "ОТМЕНА").
    pub export_dialog: Option<ExportDialog>,
    pub status: Status,
    pub params: PixelParams,
    /// Output folder/prefix of the Stage 4 export.
    pub output: OutputParams,
    /// Pixel-art result of the last generation in this mode.
    pub pixel_result: Option<GeneratedImage>,
    /// Cutout result of the last generation in this mode.
    pub cutout_result: Option<CutoutImage>,
    pub edit: Option<Edit>,
    pub dropdown_open: bool,
    pub scroll: i32,
    /// Preview zoom and pan offset shared by the source and result panes.
    pub zoom: ZoomMode,
    pub pan: Point,
    /// Keyboard focus of the Stage 5 Tab navigation.
    pub focus: Option<FocusId>,
    /// Toast queue; expired entries are dropped by [`Model::expire_toasts`].
    pub toasts: Vec<Toast>,
    /// Recently opened files, newest first (persisted in the settings file).
    pub recent: Vec<PathBuf>,
    /// File currently dragged over the window, if any. Only used for the
    /// drop hint overlay; the load itself starts on `DroppedFile`.
    pub drop_hover: Option<PathBuf>,
    /// Undo stack of parameter/output snapshots (oldest first).
    undo: Vec<Snapshot>,
    /// Redo stack filled by undo and cleared by the next change.
    redo: Vec<Snapshot>,
    /// State that matched `params`/`output` at the last committed change.
    history_base: Snapshot,
    /// File generation: bumped on every load/clear, stale loads are ignored.
    pub generation: u64,
    /// Job generation shared by generate/save requests.
    pub job_seq: u64,
}

impl Model {
    pub fn new() -> Self {
        let params = PixelParams::default();
        let output = OutputParams::default();
        Self {
            file: None,
            loading: None,
            mode: Mode::PixelArt,
            generating: None,
            cutting: None,
            saving: None,
            modal: None,
            export_dialog: None,
            status: Status::Empty,
            params,
            output: output.clone(),
            pixel_result: None,
            cutout_result: None,
            edit: None,
            dropdown_open: false,
            scroll: 0,
            zoom: ZoomMode::Fit,
            pan: Point::new(0, 0),
            focus: None,
            toasts: Vec::new(),
            recent: Vec::new(),
            drop_hover: None,
            undo: Vec::new(),
            redo: Vec::new(),
            history_base: Snapshot { params, output },
            generation: 0,
            job_seq: 0,
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode == mode {
            return;
        }
        self.commit_edit();
        self.dropdown_open = false;
        self.focus = None;
        self.mode = mode;
    }

    pub fn begin_load(&mut self, path: PathBuf) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.loading = Some(LoadJob { path });
        self.generating = None;
        self.cutting = None;
        self.saving = None;
        self.pixel_result = None;
        self.cutout_result = None;
        self.edit = None;
        self.dropdown_open = false;
        self.drop_hover = None;
        self.focus = None;
        self.zoom = ZoomMode::Fit;
        self.pan = Point::new(0, 0);
        self.status = Status::Loading;
        self.modal = None;
        self.export_dialog = None;
        self.reset_history();
        self.generation
    }

    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.file = None;
        self.loading = None;
        self.generating = None;
        self.cutting = None;
        self.saving = None;
        self.pixel_result = None;
        self.cutout_result = None;
        self.edit = None;
        self.dropdown_open = false;
        self.drop_hover = None;
        self.focus = None;
        self.zoom = ZoomMode::Fit;
        self.pan = Point::new(0, 0);
        self.modal = None;
        self.export_dialog = None;
        self.toasts.clear();
        self.status = Status::Empty;
        self.reset_history();
    }

    pub fn on_loaded(
        &mut self,
        generation: u64,
        path: PathBuf,
        image: Arc<RgbaImage>,
        meta: FileMeta,
    ) {
        if generation != self.generation {
            return;
        }
        self.push_recent(&path);
        self.file = Some(LoadedFile { path, image, meta });
        self.loading = None;
        self.generating = None;
        self.cutting = None;
        self.pixel_result = None;
        self.cutout_result = None;
        self.edit = None;
        self.pan = Point::new(0, 0);
        self.status = Status::Ready;
        self.modal = None;
        self.export_dialog = None;
    }

    pub fn on_failed(&mut self, generation: u64, message: String) {
        if generation != self.generation {
            return;
        }
        self.loading = None;
        self.status = if self.file.is_some() {
            Status::Ready
        } else {
            Status::Empty
        };
        self.modal = Some(Modal {
            title: "ОШИБКА".to_string(),
            message,
            tone: ModalTone::Error,
            buttons: ModalButtons::Ok,
            confirm: ConfirmAction::None,
        });
    }

    pub fn dismiss_modal(&mut self) {
        self.modal = None;
    }

    /// Remember a successfully opened file, newest first.
    pub fn push_recent(&mut self, path: &Path) {
        let path = path.to_path_buf();
        self.recent.retain(|known| known != &path);
        self.recent.insert(0, path);
        self.recent.truncate(crate::settings::MAX_RECENT);
    }

    /// Snapshot of the editable settings.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            params: self.params,
            output: self.output.clone(),
        }
    }

    /// Start a new undo history, e.g. after opening a file or applying the
    /// persisted settings at startup.
    pub fn reset_history(&mut self) {
        self.history_base = self.snapshot();
        self.undo.clear();
        self.redo.clear();
    }

    /// Restore the editable settings to their defaults.
    ///
    /// Only the values the settings panel owns (and the export folder/prefix)
    /// are reset. The open file, the active mode, the recent list and the
    /// window state stay as they are; the caller records the change in the
    /// undo history like any other edit.
    pub fn reset_settings(&mut self) {
        self.params = PixelParams::default();
        self.output = OutputParams::default();
        self.edit = None;
        self.dropdown_open = false;
    }

    /// Record a change that has already been applied to `params`/`output`.
    ///
    /// Called once per user event, so several field commits inside one event
    /// collapse into one undo step. A no-op change records nothing.
    pub fn commit_history(&mut self) {
        let current = self.snapshot();
        if current == self.history_base {
            return;
        }
        self.undo.push(self.history_base.clone());
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.history_base = current;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Step back one recorded change. Returns false when at the start.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.redo.push(current);
        if self.redo.len() > HISTORY_LIMIT {
            self.redo.remove(0);
        }
        self.restore(previous);
        true
    }

    /// Step forward after an undo. Returns false when the redo branch is empty.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.undo.push(current);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.restore(next);
        true
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.params = snapshot.params;
        self.output = snapshot.output;
        self.history_base = self.snapshot();
        self.edit = None;
        self.dropdown_open = false;
    }

    /// Queue a toast; the oldest one is dropped when the screen is full.
    pub fn toast(&mut self, tone: ToastTone, text: impl Into<String>) {
        if self.toasts.len() >= TOAST_LIMIT {
            self.toasts.remove(0);
        }
        self.toasts.push(Toast {
            text: text.into(),
            tone,
            expires: Instant::now() + TOAST_TTL,
        });
    }

    /// Drop expired toasts; returns true when the queue changed.
    pub fn expire_toasts(&mut self) -> bool {
        let now = Instant::now();
        let before = self.toasts.len();
        self.toasts.retain(|toast| toast.expires > now);
        self.toasts.len() != before
    }

    /// Moment the next toast disappears, used to schedule a redraw.
    pub fn next_toast_deadline(&self) -> Option<Instant> {
        self.toasts.iter().map(|toast| toast.expires).min()
    }

    /// Switch the preview scale. Panning is kept proportional between fixed
    /// percentages and reset when entering or leaving "по размеру".
    pub fn set_zoom(&mut self, mode: ZoomMode) {
        match (self.zoom, mode) {
            (ZoomMode::Percent(old), ZoomMode::Percent(new)) => {
                let factor = new as f32 / old as f32;
                self.pan = Point::new(
                    (self.pan.x as f32 * factor).round() as i32,
                    (self.pan.y as f32 * factor).round() as i32,
                );
            }
            _ => self.pan = Point::new(0, 0),
        }
        self.zoom = mode;
    }

    pub fn zoom_in(&mut self, fit_percent: Option<u32>) {
        self.set_zoom(self.zoom.next_in(fit_percent));
    }

    pub fn zoom_out(&mut self, fit_percent: Option<u32>) {
        self.set_zoom(self.zoom.next_out(fit_percent));
    }

    pub fn zoom_fit(&mut self) {
        self.set_zoom(ZoomMode::Fit);
    }

    /// Move the preview content, clamped to `limit` by the caller.
    pub fn set_pan(&mut self, pan: Point) {
        if self.zoom == ZoomMode::Fit {
            self.pan = Point::new(0, 0);
        } else {
            self.pan = pan;
        }
    }

    pub fn is_busy(&self) -> bool {
        self.loading.is_some()
            || self.generating.is_some()
            || self.cutting.is_some()
            || self.saving.is_some()
    }

    pub fn is_generating(&self) -> bool {
        self.generating.is_some()
    }

    pub fn is_cutting(&self) -> bool {
        self.cutting.is_some()
    }

    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    pub fn saving_target(&self) -> Option<SaveTarget> {
        self.saving.as_ref().map(|job| job.target)
    }

    pub fn is_dirty(&self) -> bool {
        match self.mode {
            Mode::PixelArt => self
                .pixel_result
                .as_ref()
                .is_some_and(|result| result.params != self.params),
            Mode::Cutout => self
                .cutout_result
                .as_ref()
                .is_some_and(|result| result.params != self.params.background_params()),
        }
    }

    pub fn can_generate(&self) -> bool {
        self.file.is_some() && !self.is_busy()
    }

    pub fn can_save(&self) -> bool {
        if self.is_busy() {
            return false;
        }
        match self.mode {
            Mode::PixelArt => self
                .pixel_result
                .as_ref()
                .is_some_and(|result| result.params == self.params),
            Mode::Cutout => self
                .cutout_result
                .as_ref()
                .is_some_and(|result| result.params == self.params.background_params()),
        }
    }

    pub fn begin_generate(&mut self) -> Option<GenerateRequest> {
        if self.mode != Mode::PixelArt || !self.can_generate() {
            return None;
        }
        self.commit_edit();
        self.dropdown_open = false;
        let source = Arc::clone(&self.file.as_ref()?.image);
        self.job_seq = self.job_seq.wrapping_add(1);
        let params = self.params;
        self.generating = Some(GenerateJob {
            id: self.job_seq,
            file: self.generation,
            params,
        });
        Some(GenerateRequest {
            id: self.job_seq,
            file: self.generation,
            source,
            options: params.preview_options(),
            params,
        })
    }

    pub fn on_generated(
        &mut self,
        id: u64,
        file: u64,
        sprite: Arc<RgbaImage>,
        stats: Stats,
        elapsed: Duration,
    ) {
        let accepted = self.generation == file
            && self
                .generating
                .as_ref()
                .is_some_and(|job| job.id == id && job.file == file);
        if !accepted {
            return;
        }
        let Some(job) = self.generating.take() else {
            return;
        };
        self.pixel_result = Some(GeneratedImage {
            sprite,
            stats,
            elapsed,
            params: job.params,
        });
        self.status = Status::Ready;
    }

    pub fn on_generate_failed(&mut self, id: u64, file: u64, message: String) {
        let accepted = self.generation == file
            && self
                .generating
                .as_ref()
                .is_some_and(|job| job.id == id && job.file == file);
        if !accepted {
            return;
        }
        self.generating = None;
        self.modal = Some(Modal {
            title: "ОШИБКА ГЕНЕРАЦИИ".to_string(),
            message,
            tone: ModalTone::Error,
            buttons: ModalButtons::Ok,
            confirm: ConfirmAction::None,
        });
    }

    pub fn begin_cutout(&mut self) -> Option<CutoutRequest> {
        if self.mode != Mode::Cutout || !self.can_generate() {
            return None;
        }
        self.commit_edit();
        self.dropdown_open = false;
        let source = Arc::clone(&self.file.as_ref()?.image);
        self.job_seq = self.job_seq.wrapping_add(1);
        let params = self.params.background_params();
        self.cutting = Some(CutoutJob {
            id: self.job_seq,
            file: self.generation,
            params,
        });
        Some(CutoutRequest {
            id: self.job_seq,
            file: self.generation,
            source,
            background: params.options(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn on_cutout(
        &mut self,
        id: u64,
        file: u64,
        display: Arc<RgbaImage>,
        width: u32,
        height: u32,
        stats: CleanStats,
        fully_transparent: bool,
        elapsed: Duration,
    ) {
        let accepted = self.generation == file
            && self
                .cutting
                .as_ref()
                .is_some_and(|job| job.id == id && job.file == file);
        if !accepted {
            return;
        }
        let Some(job) = self.cutting.take() else {
            return;
        };
        self.cutout_result = Some(CutoutImage {
            display,
            width,
            height,
            stats,
            fully_transparent,
            elapsed,
            params: job.params,
        });
        self.status = Status::Ready;
    }

    pub fn on_cutout_failed(&mut self, id: u64, file: u64, message: String) {
        let accepted = self.generation == file
            && self
                .cutting
                .as_ref()
                .is_some_and(|job| job.id == id && job.file == file);
        if !accepted {
            return;
        }
        self.cutting = None;
        self.modal = Some(Modal {
            title: "ОШИБКА ВЫРЕЗКИ".to_string(),
            message,
            tone: ModalTone::Error,
            buttons: ModalButtons::Ok,
            confirm: ConfirmAction::None,
        });
    }

    /// Files the active mode writes next to the source.
    pub fn save_paths(&self) -> Vec<PathBuf> {
        let Some(file) = self.file.as_ref() else {
            return Vec::new();
        };
        match self.mode {
            Mode::PixelArt => vec![
                output_path(&file.path, None, "_grid.png"),
                output_path(&file.path, None, "_sprite.png"),
            ],
            Mode::Cutout => vec![output_path(&file.path, None, "_cut.png")],
        }
    }

    /// Files the Stage 4 export writes, using the output folder/prefix set in
    /// the export dialog.
    pub fn export_paths(&self) -> Vec<PathBuf> {
        let Some(file) = self.file.as_ref() else {
            return Vec::new();
        };
        let Some(prefix) = self.export_prefix_string() else {
            return Vec::new();
        };
        match self.mode {
            Mode::PixelArt => vec![
                output_path(&file.path, Some(&prefix), "_grid.png"),
                output_path(&file.path, Some(&prefix), "_sprite.png"),
            ],
            Mode::Cutout => vec![output_path(&file.path, Some(&prefix), "_cut.png")],
        }
    }

    /// Absolute `dir/prefix` base passed to the core, per the export spec.
    pub fn export_prefix_string(&self) -> Option<String> {
        Some(self.export_base()?.to_string_lossy().into_owned())
    }

    fn export_base(&self) -> Option<PathBuf> {
        let file = self.file.as_ref()?;
        let dir = self
            .output
            .dir
            .clone()
            .or_else(|| file.path.parent().map(Path::to_path_buf))?;
        let prefix = self
            .output
            .prefix
            .clone()
            .filter(|prefix| !prefix.trim().is_empty())
            .or_else(|| {
                file.path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })?;
        Some(absolute_path(&dir.join(prefix)))
    }

    /// Folder shown in the export dialog: explicit choice, source folder, or
    /// empty.
    pub fn output_dir_display(&self) -> String {
        let dir = self.output.dir.clone().or_else(|| {
            self.file
                .as_ref()
                .and_then(|file| file.path.parent().map(Path::to_path_buf))
        });
        match dir {
            Some(dir) => absolute_path(&dir).display().to_string(),
            None => "ПАПКА ИСХОДНИКА".to_string(),
        }
    }

    /// Prefix shown in the export dialog (resolved default is the source stem).
    pub fn prefix_display(&self) -> String {
        self.output
            .prefix
            .clone()
            .or_else(|| {
                self.file.as_ref().and_then(|file| {
                    file.path
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                })
            })
            .unwrap_or_default()
    }

    /// Validation for the user-edited prefix. `None` means the default is used.
    pub fn export_prefix_error(&self) -> Option<String> {
        prefix_error(self.output.prefix.as_deref()?)
    }

    pub fn result_output_dims(&self) -> Option<(u64, u64)> {
        self.pixel_result.as_ref().map(GeneratedImage::output_dims)
    }

    /// Size of the file the active mode exports (pixels).
    pub fn export_dims(&self) -> Option<(u64, u64)> {
        match self.mode {
            Mode::PixelArt => self.result_output_dims(),
            Mode::Cutout => self
                .cutout_result
                .as_ref()
                .map(|result| (u64::from(result.width), u64::from(result.height))),
        }
    }

    /// Reasons the export must not start (PNG side limit / memory cap).
    pub fn export_blocks(&self) -> Option<String> {
        let (width, height) = self.export_dims()?;
        let noun = match self.mode {
            Mode::PixelArt => "РЕЗУЛЬТАТА",
            Mode::Cutout => "ВЫРЕЗКИ",
        };
        if width > 32_767 || height > 32_767 {
            return Some(format!(
                "РАЗМЕР {noun} {width} X {height} ПРЕВЫШАЕТ ПРЕДЕЛ 32767 ПО СТОРОНЕ.\n{}",
                match self.mode {
                    Mode::PixelArt => "УМЕНЬШИТЕ ШИРИНУ СЕТКИ ИЛИ РАЗМЕР ЯЧЕЙКИ.",
                    Mode::Cutout => "УМЕНЬШИТЕ ИСХОДНОЕ ИЗОБРАЖЕНИЕ.",
                }
            ));
        }
        let bytes = width.saturating_mul(height).saturating_mul(4);
        if bytes > 1_610_612_736 {
            return Some(format!(
                "РАЗМЕР {noun} {width} X {height} (ОКОЛО {} МБ).\nПРЕДЕЛ — 1536 МБ.\n{}",
                bytes / (1024 * 1024),
                match self.mode {
                    Mode::PixelArt => "УМЕНЬШИТЕ ШИРИНУ СЕТКИ ИЛИ РАЗМЕР ЯЧЕЙКИ.",
                    Mode::Cutout => "УМЕНЬШИТЕ ИСХОДНОЕ ИЗОБРАЖЕНИЕ.",
                }
            ));
        }
        None
    }

    /// Soft warning for results above 512 MiB; shown before the export starts.
    pub fn export_warning(&self) -> Option<String> {
        let (width, height) = self.export_dims()?;
        if width > 32_767 || height > 32_767 {
            return None;
        }
        let bytes = width.saturating_mul(height).saturating_mul(4);
        if (536_870_912..=1_610_612_736).contains(&bytes) {
            let noun = match self.mode {
                Mode::PixelArt => "РЕЗУЛЬТАТА",
                Mode::Cutout => "ВЫРЕЗКИ",
            };
            return Some(format!(
                "РАЗМЕР {noun} {width} X {height} (ОКОЛО {} МБ БУФЕРА).\nЭКСПОРТ МОЖЕТ ЗАНЯТЬ МНОГО ВРЕМЕНИ И ПАМЯТИ.\nПРОДОЛЖИТЬ?",
                bytes / (1024 * 1024)
            ));
        }
        None
    }

    pub fn show_error(&mut self, title: &str, message: String) {
        self.modal = Some(Modal {
            title: title.to_string(),
            message,
            tone: ModalTone::Error,
            buttons: ModalButtons::Ok,
            confirm: ConfirmAction::None,
        });
    }

    pub fn begin_write(&mut self, target: SaveTarget) -> Option<SaveRequest> {
        if !self.can_save() {
            return None;
        }
        let path = self.file.as_ref()?.path.clone();
        let prefix = match target {
            SaveTarget::Quick => None,
            SaveTarget::Export => Some(self.export_prefix_string()?),
        };
        self.job_seq = self.job_seq.wrapping_add(1);
        let id = self.job_seq;
        let file = self.generation;
        match self.mode {
            Mode::PixelArt => {
                let params = self.pixel_result.as_ref()?.params;
                self.saving = Some(SaveJob {
                    id,
                    file,
                    kind: Mode::PixelArt,
                    target,
                });
                Some(SaveRequest::Pixel {
                    id,
                    file,
                    path,
                    prefix,
                    options: params.options(),
                })
            }
            Mode::Cutout => {
                let params = self.cutout_result.as_ref()?.params;
                self.saving = Some(SaveJob {
                    id,
                    file,
                    kind: Mode::Cutout,
                    target,
                });
                Some(SaveRequest::Cutout {
                    id,
                    file,
                    path,
                    prefix,
                    background: params.options(),
                })
            }
        }
    }

    pub fn on_saved(
        &mut self,
        id: u64,
        file: u64,
        grid_path: PathBuf,
        sprite_path: PathBuf,
        stats: Stats,
        elapsed: Duration,
    ) {
        let target = self
            .saving
            .as_ref()
            .filter(|job| {
                self.generation == file
                    && job.id == id
                    && job.file == file
                    && job.kind == Mode::PixelArt
            })
            .map(|job| job.target);
        let Some(target) = target else {
            return;
        };
        self.saving = None;
        let title = match target {
            SaveTarget::Quick => "СОХРАНЕНО",
            SaveTarget::Export => "ЭКСПОРТ ВЫПОЛНЕН",
        };
        let folder = absolute_path(&grid_path)
            .parent()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.toast(
            ToastTone::Info,
            format!(
                "{title}: {} + {}\n{} X {} | {} ЦВЕТОВ | {} МС\n{}",
                file_name_of(&grid_path),
                file_name_of(&sprite_path),
                stats.output_width,
                stats.output_height,
                stats.palette_colors,
                elapsed.as_millis(),
                folder
            ),
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn on_cutout_saved(
        &mut self,
        id: u64,
        file: u64,
        path: PathBuf,
        width: u32,
        height: u32,
        stats: CleanStats,
        fully_transparent: bool,
        elapsed: Duration,
    ) {
        let target = self
            .saving
            .as_ref()
            .filter(|job| {
                self.generation == file
                    && job.id == id
                    && job.file == file
                    && job.kind == Mode::Cutout
            })
            .map(|job| job.target);
        let Some(target) = target else {
            return;
        };
        self.saving = None;
        let removed = stats.background.as_ref().map_or_else(
            || "ФОН НЕ ОБРАБОТАН".to_string(),
            |background| format!("УДАЛЕНО {:.1}% ФОНА", background.removed_percent),
        );
        let title = match target {
            SaveTarget::Quick => "СОХРАНЕНО",
            SaveTarget::Export => "ВЫРЕЗКА ЭКСПОРТИРОВАНА",
        };
        let folder = absolute_path(&path)
            .parent()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let mut message = format!(
            "{title}: {}\n{} X {} | {removed} | {} МС\n{folder}",
            file_name_of(&path),
            width,
            height,
            elapsed.as_millis()
        );
        if fully_transparent {
            message.push_str("\nВНИМАНИЕ: РЕЗУЛЬТАТ ПОЛНОСТЬЮ ПРОЗРАЧНЫЙ");
        }
        self.toast(
            if fully_transparent {
                ToastTone::Error
            } else {
                ToastTone::Info
            },
            message,
        );
    }

    pub fn on_save_failed(&mut self, id: u64, file: u64, message: String) {
        let accepted = self.generation == file
            && self
                .saving
                .as_ref()
                .is_some_and(|job| job.id == id && job.file == file);
        if !accepted {
            return;
        }
        self.saving = None;
        self.modal = Some(Modal {
            title: "ОШИБКА СОХРАНЕНИЯ".to_string(),
            message,
            tone: ModalTone::Error,
            buttons: ModalButtons::Ok,
            confirm: ConfirmAction::None,
        });
    }

    pub fn overwrite_modal(&mut self, target: SaveTarget) {
        let paths = match target {
            SaveTarget::Quick => self.save_paths(),
            SaveTarget::Export => self.export_paths(),
        };
        if paths.is_empty() {
            return;
        }
        let list = paths
            .iter()
            .map(|path| absolute_path(path).display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        self.modal = Some(Modal {
            title: "ПЕРЕЗАПИСАТЬ ФАЙЛЫ?".to_string(),
            message: format!("ФАЙЛЫ УЖЕ СУЩЕСТВУЮТ:\n{list}\n\nПЕРЕЗАПИСАТЬ?"),
            tone: ModalTone::Warning,
            buttons: ModalButtons::OkCancel,
            confirm: match target {
                SaveTarget::Quick => ConfirmAction::SaveOverwrite,
                SaveTarget::Export => ConfirmAction::ExportOverwrite,
            },
        });
    }

    /// Soft size warning shown before a large export/save starts.
    pub fn size_warning_modal(&mut self, target: SaveTarget) {
        let Some(message) = self.export_warning() else {
            return;
        };
        self.modal = Some(Modal {
            title: "БОЛЬШОЙ РЕЗУЛЬТАТ".to_string(),
            message,
            tone: ModalTone::Warning,
            buttons: ModalButtons::OkCancel,
            confirm: match target {
                SaveTarget::Quick => ConfirmAction::SaveLarge,
                SaveTarget::Export => ConfirmAction::ExportLarge,
            },
        });
    }

    /// Shown when the window is closed while a worker operation is running.
    pub fn close_wait_modal(&mut self) {
        self.modal = Some(Modal {
            title: "ИДЁТ ОБРАБОТКА".to_string(),
            message: "ТЕКУЩАЯ ОПЕРАЦИЯ ЕЩЁ НЕ ЗАВЕРШЕНА.\nДОЖДАТЬСЯ ОКОНЧАНИЯ И ВЫЙТИ?".to_string(),
            tone: ModalTone::Warning,
            buttons: ModalButtons::OkCancel,
            confirm: ConfirmAction::WaitAndExit,
        });
    }

    pub fn set_output_dir(&mut self, dir: PathBuf) {
        self.output.dir = Some(dir);
    }

    /// Open the export dialog. The prefix field starts selected so typing
    /// replaces the current name; the folder is picked with "ОБЗОР".
    pub fn open_export_dialog(&mut self) {
        if self.file.is_none() || self.is_busy() || self.export_dialog.is_some() {
            return;
        }
        self.commit_edit();
        self.dropdown_open = false;
        self.export_dialog = Some(ExportDialog {
            base: self.output.clone(),
        });
        self.begin_edit(Field::OutputPrefix);
        self.focus = Some(FocusId::Field(Field::OutputPrefix));
    }

    /// Close the export dialog, keeping the folder and prefix it applied.
    pub fn finish_export_dialog(&mut self) {
        self.export_dialog = None;
        self.edit = None;
        self.focus = None;
    }

    /// Close the export dialog and drop the folder/prefix changes.
    pub fn cancel_export_dialog(&mut self) {
        if let Some(dialog) = self.export_dialog.take() {
            self.output = dialog.base;
        }
        self.edit = None;
        self.focus = None;
    }

    pub fn export_dialog_open(&self) -> bool {
        self.export_dialog.is_some()
    }

    /// Prefix text as it currently reads: the field being typed or the applied
    /// value. The default is the source file stem.
    pub fn export_prefix_text(&self) -> String {
        match &self.edit {
            Some(edit) if edit.field == Field::OutputPrefix => edit.text.clone(),
            _ => self.prefix_display(),
        }
    }

    /// Why "СОХРАНИТЬ" in the export dialog cannot start the export, if so.
    /// Validated on the text in the field, not on the committed value.
    pub fn export_dialog_error(&self) -> Option<String> {
        prefix_error(&self.export_prefix_text())
    }

    /// File names the export dialog is about to write, resolved for live
    /// feedback while the prefix is being typed.
    pub fn export_preview_names(&self) -> Vec<String> {
        let prefix = self.export_prefix_text().trim().to_string();
        if prefix.is_empty() {
            return Vec::new();
        }
        match self.mode {
            Mode::PixelArt => vec![
                format!("{prefix}_grid.png"),
                format!("{prefix}_sprite.png"),
            ],
            Mode::Cutout => vec![format!("{prefix}_cut.png")],
        }
    }

    pub fn field_text(&self, field: Field) -> String {
        match field {
            Field::Width => self.params.width.to_string(),
            Field::Colors => self.params.colors.to_string(),
            Field::Cell => self.params.cell.to_string(),
            Field::GridOpacity => self.params.grid_opacity.to_string(),
            Field::GridColor => format_hex(self.params.grid_color),
            Field::AlphaThreshold => self.params.alpha_threshold.to_string(),
            Field::FillColor => format_hex(self.params.fill_color),
            Field::Despeckle => self.params.despeckle.to_string(),
            Field::BgColor => format_hex(self.params.bg_color),
            Field::BgTolerance => self.params.bg_tolerance.to_string(),
            Field::BgGrow => self.params.bg_grow.to_string(),
            Field::BgDefringe => self.params.bg_defringe.to_string(),
            Field::HoleArea => format_pct(self.params.hole_area_pct),
            Field::OutputPrefix => self.prefix_display(),
        }
    }

    pub fn display_text(&self, field: Field) -> String {
        match &self.edit {
            Some(edit) if edit.field == field => edit.text.clone(),
            _ => self.field_text(field),
        }
    }

    pub fn display_caret(&self, field: Field) -> Option<usize> {
        match &self.edit {
            Some(edit) if edit.field == field => Some(edit.caret),
            _ => None,
        }
    }

    pub fn display_invalid(&self, field: Field) -> bool {
        match &self.edit {
            Some(edit) if edit.field == field => edit.invalid(),
            _ => false,
        }
    }

    pub fn field_color(&self, field: Field) -> Color {
        match field {
            Field::GridColor => self.params.grid_color,
            Field::FillColor => self.params.fill_color,
            Field::BgColor => self.params.bg_color,
            _ => [0, 0, 0],
        }
    }

    pub fn field_enabled(&self, field: Field) -> bool {
        if self.file.is_none() {
            return false;
        }
        if self.mode == Mode::Cutout {
            // Cutout always removes the background, so its fields only depend
            // on the "auto" switches.
            return match field {
                Field::BgColor => !self.params.bg_auto_color,
                Field::BgTolerance | Field::BgGrow | Field::HoleArea => true,
                Field::BgDefringe => !self.params.bg_auto_defringe,
                Field::OutputPrefix => true,
                _ => false,
            };
        }
        match field {
            Field::FillColor => self.params.fill_enabled,
            Field::BgColor => self.params.remove_bg && !self.params.bg_auto_color,
            Field::BgTolerance | Field::BgGrow => self.params.remove_bg,
            Field::BgDefringe => self.params.remove_bg && !self.params.bg_auto_defringe,
            Field::HoleArea => self.params.remove_bg,
            Field::OutputPrefix => true,
            _ => true,
        }
    }

    pub fn toggle_enabled(&self, toggle: Toggle) -> bool {
        if self.file.is_none() {
            return false;
        }
        if self.mode == Mode::Cutout {
            return matches!(
                toggle,
                Toggle::BgAutoColor
                    | Toggle::BgAutoDefringe
                    | Toggle::ThinGaps
                    | Toggle::RemoveHoles
            );
        }
        match toggle {
            Toggle::Fill => true,
            Toggle::RemoveBg => true,
            Toggle::BgAutoColor
            | Toggle::BgAutoDefringe
            | Toggle::ThinGaps
            | Toggle::RemoveHoles => self.params.remove_bg,
        }
    }

    pub fn toggle_checked(&self, toggle: Toggle) -> bool {
        match toggle {
            Toggle::Fill => self.params.fill_enabled,
            Toggle::RemoveBg => self.params.remove_bg,
            Toggle::BgAutoColor => self.params.bg_auto_color,
            Toggle::BgAutoDefringe => self.params.bg_auto_defringe,
            Toggle::ThinGaps => self.params.thin_gaps,
            Toggle::RemoveHoles => self.params.remove_holes,
        }
    }

    pub fn begin_edit(&mut self, field: Field) {
        if !self.field_enabled(field) {
            return;
        }
        if self.edit.as_ref().is_some_and(|edit| edit.field == field) {
            return;
        }
        self.commit_edit();
        let text = self.field_text(field);
        let count = text.chars().count();
        self.edit = Some(Edit {
            field,
            caret: count,
            text,
            select_all: true,
        });
        self.dropdown_open = false;
    }

    pub fn display_select_all(&self, field: Field) -> bool {
        match &self.edit {
            Some(edit) if edit.field == field => edit.select_all,
            _ => false,
        }
    }

    pub fn cancel_edit(&mut self) {
        self.edit = None;
    }

    pub fn commit_edit(&mut self) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        if !self.field_enabled(edit.field) {
            return;
        }
        match parse_field(edit.field, &edit.text) {
            Some(ParsedField::Int(value)) => self.apply_int(edit.field, value),
            Some(ParsedField::Float(value)) => {
                self.params.hole_area_pct = round_pct(value.clamp(0.001, 10.0));
            }
            Some(ParsedField::Hex(color)) => self.apply_hex(edit.field, color),
            Some(ParsedField::Text(value)) => self.output.prefix = Some(value),
            None => {}
        }
    }

    pub fn push_char(&mut self, ch: char) {
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        let field = edit.field;
        let ch = if field == Field::OutputPrefix {
            if ch.is_control() || ch == '/' || ch == '\\' {
                return;
            }
            if cfg!(windows) && matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                return;
            }
            ch
        } else if field.is_hex() {
            if ch == '#' {
                return;
            }
            let upper = ch.to_ascii_uppercase();
            if !upper.is_ascii_hexdigit() {
                return;
            }
            upper
        } else if field == Field::HoleArea {
            match ch {
                '0'..='9' => ch,
                '.' | ',' => '.',
                _ => return,
            }
        } else {
            if !ch.is_ascii_digit() {
                return;
            }
            ch
        };
        let limit = if field == Field::OutputPrefix { 80 } else { 6 };
        if edit.select_all {
            edit.text.clear();
            edit.caret = 0;
            edit.select_all = false;
        }
        if edit.text.chars().count() >= limit {
            return;
        }
        let caret = edit.caret.min(edit.text.chars().count());
        let byte = edit
            .text
            .char_indices()
            .nth(caret)
            .map(|(index, _)| index)
            .unwrap_or(edit.text.len());
        edit.text.insert(byte, ch);
        edit.caret = caret + 1;
    }

    pub fn backspace(&mut self) {
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        if edit.select_all {
            edit.text.clear();
            edit.caret = 0;
            edit.select_all = false;
            return;
        }
        let count = edit.text.chars().count();
        if edit.caret == 0 || count == 0 {
            return;
        }
        let caret = edit.caret.min(count);
        let start = edit
            .text
            .char_indices()
            .nth(caret - 1)
            .map(|(index, _)| index)
            .unwrap_or(0);
        let end = edit
            .text
            .char_indices()
            .nth(caret)
            .map(|(index, _)| index)
            .unwrap_or(edit.text.len());
        edit.text.replace_range(start..end, "");
        edit.caret = caret - 1;
    }

    pub fn delete_forward(&mut self) {
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        if edit.select_all {
            edit.text.clear();
            edit.caret = 0;
            edit.select_all = false;
            return;
        }
        let count = edit.text.chars().count();
        if edit.caret >= count {
            return;
        }
        let start = edit
            .text
            .char_indices()
            .nth(edit.caret)
            .map(|(index, _)| index)
            .unwrap_or(edit.text.len());
        let end = edit
            .text
            .char_indices()
            .nth(edit.caret + 1)
            .map(|(index, _)| index)
            .unwrap_or(edit.text.len());
        edit.text.replace_range(start..end, "");
    }

    pub fn move_caret(&mut self, delta: i32) {
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        edit.select_all = false;
        let count = edit.text.chars().count();
        let caret = (edit.caret as i32 + delta).clamp(0, count as i32) as usize;
        edit.caret = caret;
    }

    pub fn caret_home(&mut self) {
        if let Some(edit) = self.edit.as_mut() {
            edit.caret = 0;
            edit.select_all = false;
        }
    }

    pub fn caret_end(&mut self) {
        if let Some(edit) = self.edit.as_mut() {
            edit.caret = edit.text.chars().count();
            edit.select_all = false;
        }
    }

    pub fn select_all_edit(&mut self) {
        if let Some(edit) = self.edit.as_mut() {
            edit.select_all = true;
            edit.caret = edit.text.chars().count();
        }
    }

    pub fn step_field(&mut self, field: Field, delta: i32) {
        if !self.field_enabled(field) {
            return;
        }
        if field == Field::HoleArea {
            let value = (self.params.hole_area_pct + delta as f32 * 0.01).clamp(0.001, 10.0);
            self.params.hole_area_pct = round_pct(value);
            return;
        }
        let current = match field {
            Field::Width => self.params.width as i64,
            Field::Colors => self.params.colors as i64,
            Field::Cell => self.params.cell as i64,
            Field::GridOpacity => self.params.grid_opacity as i64,
            Field::AlphaThreshold => self.params.alpha_threshold as i64,
            Field::Despeckle => self.params.despeckle as i64,
            Field::BgTolerance => self.params.bg_tolerance as i64,
            Field::BgGrow => self.params.bg_grow as i64,
            Field::BgDefringe => self.params.bg_defringe as i64,
            _ => return,
        };
        self.apply_int(field, current + delta as i64);
    }

    pub fn step_edit(&mut self, delta: i32) {
        let Some(field) = self.edit.as_ref().map(|edit| edit.field) else {
            return;
        };
        self.step_field(field, delta);
        let text = self.field_text(field);
        if let Some(edit) = self.edit.as_mut() {
            edit.caret = text.chars().count();
            edit.text = text;
            edit.select_all = false;
        }
    }

    pub fn toggle(&mut self, toggle: Toggle) {
        if !self.toggle_enabled(toggle) {
            return;
        }
        match toggle {
            Toggle::Fill => self.params.fill_enabled = !self.params.fill_enabled,
            Toggle::RemoveBg => self.params.remove_bg = !self.params.remove_bg,
            Toggle::BgAutoColor => self.params.bg_auto_color = !self.params.bg_auto_color,
            Toggle::BgAutoDefringe => {
                self.params.bg_auto_defringe = !self.params.bg_auto_defringe;
            }
            Toggle::ThinGaps => self.params.thin_gaps = !self.params.thin_gaps,
            Toggle::RemoveHoles => self.params.remove_holes = !self.params.remove_holes,
        }
        if let Some(edit) = &self.edit
            && !self.field_enabled(edit.field)
        {
            self.edit = None;
        }
    }

    /// "ВЗЯТЬ ЦВЕТ С УГЛА": use the top-left source pixel as the manual
    /// background colour. Returns the sampled RGB, or `None` without a file.
    ///
    /// The auto colour switch is turned off so the sampled colour is actually
    /// used by the next conversion; the "удалить фон" switch is not changed,
    /// because the button lives inside the already enabled background block.
    pub fn pick_corner_color(&mut self) -> Option<Color> {
        let file = self.file.as_ref()?;
        let (width, height) = file.image.dimensions();
        if width == 0 || height == 0 {
            return None;
        }
        let px = file.image.get_pixel(0, 0).0;
        let color: Color = [px[0], px[1], px[2]];
        self.params.bg_color = color;
        self.params.bg_auto_color = false;
        Some(color)
    }

    pub fn apply_filter(&mut self, filter: Filter) {
        self.params.filter = filter;
        self.dropdown_open = false;
    }

    fn apply_int(&mut self, field: Field, value: i64) {
        let Some((min, max)) = int_bounds(field) else {
            return;
        };
        let value = value.clamp(min, max);
        match field {
            Field::Width => self.params.width = value as u32,
            Field::Colors => self.params.colors = value as u16,
            Field::Cell => self.params.cell = value as u32,
            Field::GridOpacity => self.params.grid_opacity = value as u8,
            Field::AlphaThreshold => self.params.alpha_threshold = value as u8,
            Field::Despeckle => self.params.despeckle = value as u32,
            Field::BgTolerance => self.params.bg_tolerance = value as u8,
            Field::BgGrow => self.params.bg_grow = value as u32,
            Field::BgDefringe => self.params.bg_defringe = value as u32,
            _ => {}
        }
    }

    fn apply_hex(&mut self, field: Field, color: Color) {
        match field {
            Field::GridColor => self.params.grid_color = color,
            Field::FillColor => self.params.fill_color = color,
            Field::BgColor => self.params.bg_color = color,
            _ => {}
        }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

pub fn filter_label(filter: Filter) -> &'static str {
    match filter {
        Filter::Lanczos => "LANCZOS",
        Filter::Catmull => "CATMULL",
        Filter::Triangle => "TRIANGLE",
        Filter::Gaussian => "GAUSSIAN",
        Filter::Nearest => "NEAREST",
        Filter::Box => "BOX",
        Filter::Mode => "MODE",
    }
}

pub fn filter_hint(filter: Filter) -> &'static str {
    match filter {
        Filter::Lanczos => "LANCZOS: САМЫЙ ЧЁТКИЙ, ВОЗМОЖНЫ ОРЕОЛЫ",
        Filter::Catmull => "CATMULL: ЧЁТКИЙ, ОРЕОЛЫ СЛАБЕЕ",
        Filter::Triangle => "TRIANGLE: МЯГКИЙ, БЕЗ ОРЕОЛОВ",
        Filter::Gaussian => "GAUSSIAN: САМЫЙ РАЗМЫТЫЙ",
        Filter::Nearest => "NEAREST: ОДИН ПИКСЕЛЬ, ЖЁСТКИЕ КРАЯ",
        Filter::Box => "BOX: СРЕДНЕЕ ЦВЕТОВ БЛОКА",
        Filter::Mode => "MODE: САМЫЙ ЧАСТЫЙ ЦВЕТ БЛОКА",
    }
}

pub fn filter_values() -> [Filter; 7] {
    [
        Filter::Lanczos,
        Filter::Catmull,
        Filter::Triangle,
        Filter::Gaussian,
        Filter::Nearest,
        Filter::Box,
        Filter::Mode,
    ]
}

impl Field {
    pub fn is_hex(self) -> bool {
        matches!(self, Field::GridColor | Field::FillColor | Field::BgColor)
    }
}

enum ParsedField {
    Int(i64),
    Float(f32),
    Hex(Color),
    Text(String),
}

fn parse_field(field: Field, text: &str) -> Option<ParsedField> {
    let text = text.trim();
    if field == Field::OutputPrefix {
        if text.chars().any(char::is_control) || prefix_error(text).is_some() {
            return None;
        }
        Some(ParsedField::Text(text.to_string()))
    } else if field.is_hex() {
        parse_hex(text).ok().map(ParsedField::Hex)
    } else if field == Field::HoleArea {
        text.parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .map(ParsedField::Float)
    } else {
        text.parse::<i64>().ok().map(ParsedField::Int)
    }
}

/// Rules the export prefix must satisfy: a plain file name, no path.
fn prefix_error(prefix: &str) -> Option<String> {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return Some("ПРЕФИКС НЕ МОЖЕТ БЫТЬ ПУСТЫМ.".to_string());
    }
    if prefix.chars().any(|ch| ch == '/' || ch == '\\') {
        return Some(
            "ПРЕФИКС НЕ ДОЛЖЕН СОДЕРЖАТЬ ПУТЬ.\nПАПКУ ВЫБЕРИТЕ КНОПКОЙ \"ОБЗОР\".".to_string(),
        );
    }
    if cfg!(windows) && prefix.chars().any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return Some(
            "ПРЕФИКС СОДЕРЖИТ НЕДОПУСТИМЫЕ ДЛЯ WINDOWS СИМВОЛЫ:\n< > : \" | ? *".to_string(),
        );
    }
    None
}

fn int_bounds(field: Field) -> Option<(i64, i64)> {
    Some(match field {
        Field::Width => (1, 512),
        Field::Colors => (2, 256),
        Field::Cell => (2, 64),
        Field::GridOpacity => (0, 100),
        Field::AlphaThreshold => (1, 255),
        Field::Despeckle => (0, 32),
        Field::BgTolerance => (0, 255),
        Field::BgGrow => (0, 16),
        Field::BgDefringe => (0, 64),
        _ => return None,
    })
}

fn format_hex(color: Color) -> String {
    format!("{:02X}{:02X}{:02X}", color[0], color[1], color[2])
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn format_pct(value: f32) -> String {
    let mut text = format!("{value:.3}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text.is_empty() {
        "0".to_string()
    } else {
        text
    }
}

fn round_pct(value: f32) -> f32 {
    (value * 1000.0).round() / 1000.0
}

/// Absolute path for user-facing dialogs; `output_path` may return a relative
/// path when the source itself was opened with a relative path.
fn absolute_path(path: &std::path::Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Model with one loaded 1x1 file, enough for the export dialog.
    fn model_with_file() -> Model {
        let mut model = Model::new();
        model.file = Some(LoadedFile {
            path: PathBuf::from("C:/art/source.png"),
            image: Arc::new(RgbaImage::new(1, 1)),
            meta: FileMeta {
                format: "PNG".to_string(),
                bytes: 0,
                width: 1,
                height: 1,
            },
        });
        model
    }

    #[test]
    fn prefix_rules_reject_empty_and_paths() {
        assert!(prefix_error("").is_some());
        assert!(prefix_error("   ").is_some());
        assert!(prefix_error("a/b").is_some());
        assert!(prefix_error("a\\b").is_some());
        assert!(prefix_error("my_art-1").is_none());
    }

    #[test]
    fn prefix_field_uses_the_export_rules() {
        let parsed = parse_field(Field::OutputPrefix, " art ");
        assert!(matches!(parsed, Some(ParsedField::Text(text)) if text == "art"));
        assert!(parse_field(Field::OutputPrefix, "   ").is_none());
        assert!(parse_field(Field::OutputPrefix, "a/b").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn prefix_rules_reject_windows_specials() {
        assert!(prefix_error("a:b").is_some());
        assert!(prefix_error("a*b").is_some());
        assert!(prefix_error("a?b").is_some());
    }

    #[test]
    fn export_dialog_previews_the_source_stem() {
        let mut model = model_with_file();
        model.open_export_dialog();
        assert!(model.export_dialog_open());
        assert_eq!(model.export_prefix_text(), "source");
        assert_eq!(
            model.export_preview_names(),
            vec!["source_grid.png", "source_sprite.png"]
        );
        model.set_mode(Mode::Cutout);
        model.open_export_dialog();
        assert_eq!(model.export_preview_names(), vec!["source_cut.png"]);
    }

    #[test]
    fn export_dialog_cancel_restores_the_previous_output() {
        let mut model = model_with_file();
        model.output.prefix = Some("old".to_string());
        model.output.dir = Some(PathBuf::from("C:/art/out"));
        model.open_export_dialog();
        // The field starts selected, so typing replaces the whole name.
        model.push_char('n');
        model.cancel_export_dialog();
        assert!(!model.export_dialog_open());
        assert_eq!(model.output.prefix.as_deref(), Some("old"));
        assert_eq!(model.output.dir, Some(PathBuf::from("C:/art/out")));
        assert!(model.edit.is_none());
    }

    #[test]
    fn export_dialog_save_keeps_the_new_prefix() {
        let mut model = model_with_file();
        model.open_export_dialog();
        model.push_char('n');
        model.push_char('e');
        model.push_char('w');
        assert_eq!(model.export_prefix_text(), "new");
        assert!(model.export_dialog_error().is_none());
        model.commit_edit();
        model.finish_export_dialog();
        assert!(!model.export_dialog_open());
        assert_eq!(model.output.prefix.as_deref(), Some("new"));
        assert_eq!(
            model.export_preview_names(),
            vec!["new_grid.png", "new_sprite.png"]
        );
    }

    #[test]
    fn empty_prefix_blocks_the_export_dialog() {
        let mut model = model_with_file();
        model.open_export_dialog();
        model.backspace();
        assert_eq!(model.export_prefix_text(), "");
        assert!(model.export_dialog_error().is_some());
        // Committing an invalid name keeps the dialog usable and the old value.
        model.commit_edit();
        assert_eq!(model.output.prefix, None);
        assert!(model.export_dialog_open());
    }

    #[test]
    fn reset_settings_returns_the_editable_values_to_defaults() {
        let mut model = model_with_file();
        model.params.width = 96;
        model.params.colors = 8;
        model.params.remove_bg = true;
        model.output.prefix = Some("art".to_string());
        model.output.dir = Some(PathBuf::from("C:/art/out"));
        model.begin_edit(Field::Width);
        model.dropdown_open = true;

        model.reset_settings();

        assert_eq!(model.params, PixelParams::default());
        assert_eq!(model.output, OutputParams::default());
        assert!(model.edit.is_none());
        assert!(!model.dropdown_open);
        // File context and recent files are not settings; reset keeps them.
        assert!(model.file.is_some());
    }

    #[test]
    fn reset_settings_is_undoable() {
        let mut model = Model::new();
        model.params.width = 96;
        model.commit_history();

        model.reset_settings();
        model.commit_history();
        assert!(model.can_undo());

        model.undo();
        assert_eq!(model.params.width, 96);
        assert!(model.can_redo());
    }
}
