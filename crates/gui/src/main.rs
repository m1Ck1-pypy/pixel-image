//! `pixel-image-gui` — retro desktop front-end for `pixel-image-core`.
//!
//! Two modes: pixel-art edits the CLI parameters and saves `_grid.png` /
//! `_sprite.png`; cutout removes the background at the original resolution and
//! saves `_cut.png`. Both modes can write next to the source or export to a
//! chosen folder with a custom prefix. The editor also keeps its settings,
//! recent files and the preview zoom in a JSON document in the user profile.

// A GUI process must not allocate a console window in release builds. Debug
// builds keep the console so diagnostics stay visible.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, bail};
use pixel_image_core::{BackgroundStats, CleanStats};
use pixel_image_gui::{
    app::App,
    model::{
        CutoutImage, CutoutJob, Field, GenerateJob, GeneratedImage, Mode, Model, SaveJob,
        SaveTarget, ToastTone,
    },
    view,
    worker::{FileMeta, UserEvent, shrink_for_display},
};
use pixel_image_ui::{Canvas, Font, Metrics, UiState};
use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    #[cfg(all(windows, not(debug_assertions)))]
    attach_parent_console();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--dump-ui") {
        let output = args
            .get(1)
            .context("--dump-ui требует путь к PNG: --dump-ui <OUT.png> [IMAGE]")?;
        let image = args.get(2).map(PathBuf::from);
        dump_ui(Path::new(output), image.as_deref())?;
        return Ok(());
    }

    let initial = parse_args(&args)?;
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(initial, proxy);
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn parse_args(args: &[String]) -> anyhow::Result<Option<PathBuf>> {
    let Some(first) = args.first() else {
        return Ok(None);
    };
    match first.as_str() {
        "-h" | "--help" => {
            print_help();
            std::process::exit(0);
        }
        "-V" | "--version" => {
            write_out(&format!("pixel-image-gui {}\n", env!("CARGO_PKG_VERSION")));
            std::process::exit(0);
        }
        _ if first.starts_with('-') => {
            bail!("неизвестный аргумент {first:?}; попробуйте --help");
        }
        _ => {}
    }
    if args.len() > 1 {
        bail!("можно указать только один файл");
    }
    Ok(Some(PathBuf::from(first)))
}

#[derive(Default)]
struct DumpFlags {
    mode: Option<Mode>,
    generated: bool,
    cutout: bool,
    cutout_empty: bool,
    dirty: bool,
    generating: bool,
    saving: bool,
    saved: bool,
    confirm: bool,
    exporting: bool,
    exported: bool,
    export_confirm: bool,
    export_big: bool,
    export_dialog: bool,
    closing: bool,
    cell: Option<u32>,
    prefix: Option<String>,
    filter_open: bool,
    edit: bool,
    remove_bg: bool,
    fill: bool,
    drop: bool,
    pipette: bool,
    undo_state: bool,
    redo_state: bool,
    scroll: i32,
    size: Option<(u32, u32)>,
}

impl DumpFlags {
    fn from_env() -> Self {
        let env = |name: &str| std::env::var_os(name).is_some();
        let mode = match std::env::var("PIXEL_IMAGE_UI_MODE") {
            Ok(value) if value.eq_ignore_ascii_case("cutout") || value == "1" => Some(Mode::Cutout),
            Ok(value) if value.eq_ignore_ascii_case("pixel") || value == "0" => {
                Some(Mode::PixelArt)
            }
            _ => None,
        };
        let dirty = env("PIXEL_IMAGE_UI_DIRTY");
        let scroll = std::env::var("PIXEL_IMAGE_UI_SCROLL")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        let size = std::env::var("PIXEL_IMAGE_UI_SIZE").ok().and_then(|value| {
            let (width, height) = value.split_once('x')?;
            Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
        });
        let cell = std::env::var("PIXEL_IMAGE_UI_CELL")
            .ok()
            .and_then(|value| value.parse::<u32>().ok());
        let prefix = std::env::var("PIXEL_IMAGE_UI_PREFIX").ok();
        Self {
            mode,
            generated: env("PIXEL_IMAGE_UI_GENERATED"),
            cutout: env("PIXEL_IMAGE_UI_CUTOUT"),
            cutout_empty: env("PIXEL_IMAGE_UI_CUTOUT_EMPTY"),
            dirty,
            generating: env("PIXEL_IMAGE_UI_GENERATING"),
            saving: env("PIXEL_IMAGE_UI_SAVING"),
            saved: env("PIXEL_IMAGE_UI_SAVED"),
            confirm: env("PIXEL_IMAGE_UI_CONFIRM"),
            exporting: env("PIXEL_IMAGE_UI_EXPORTING"),
            exported: env("PIXEL_IMAGE_UI_EXPORTED"),
            export_confirm: env("PIXEL_IMAGE_UI_EXPORT_CONFIRM"),
            export_big: env("PIXEL_IMAGE_UI_EXPORT_BIG"),
            export_dialog: env("PIXEL_IMAGE_UI_EXPORT_DIALOG"),
            closing: env("PIXEL_IMAGE_UI_CLOSE"),
            cell,
            prefix,
            filter_open: env("PIXEL_IMAGE_UI_FILTER_OPEN"),
            edit: env("PIXEL_IMAGE_UI_EDIT"),
            remove_bg: env("PIXEL_IMAGE_UI_REMOVE_BG"),
            fill: env("PIXEL_IMAGE_UI_FILL"),
            drop: env("PIXEL_IMAGE_UI_DROP"),
            pipette: env("PIXEL_IMAGE_UI_PIPETTE"),
            undo_state: env("PIXEL_IMAGE_UI_UNDO"),
            redo_state: env("PIXEL_IMAGE_UI_REDO"),
            scroll,
            size,
        }
    }

    fn cutout_mode(&self) -> bool {
        self.cutout || self.cutout_empty || self.mode == Some(Mode::Cutout)
    }

    fn active_mode(&self) -> Mode {
        if self.cutout_mode() {
            Mode::Cutout
        } else {
            Mode::PixelArt
        }
    }

    fn wants_pixel_result(&self) -> bool {
        !self.cutout_mode()
            && (self.generated
                || self.dirty
                || self.filter_open
                || self.remove_bg
                || self.fill
                || self.generating
                || self.saving
                || self.saved
                || self.confirm
                || self.exporting
                || self.exported
                || self.export_confirm
                || self.export_big
                || self.export_dialog)
    }

    fn wants_cutout_result(&self) -> bool {
        self.cutout_mode()
            && (self.cutout
                || self.cutout_empty
                || self.generated
                || self.dirty
                || self.generating
                || self.saving
                || self.saved
                || self.confirm
                || self.exporting
                || self.exported
                || self.export_confirm
                || self.export_big
                || self.export_dialog)
    }
}

/// Render a deterministic UI frame without opening a window.
///
/// Useful for visual QA of the retro layout. The optional image is rendered as
/// the loaded state; without it the empty state is dumped. Extra states are
/// selected with `PIXEL_IMAGE_UI_*` environment variables.
fn dump_ui(output: &Path, image_path: Option<&Path>) -> anyhow::Result<()> {
    // Optional QA hook for checking the high-DPI layout without changing the
    // monitor scaling: PIXEL_IMAGE_UI_SCALE=2.
    let ui_scale = std::env::var("PIXEL_IMAGE_UI_SCALE")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|scale| *scale >= 1)
        .unwrap_or(1);
    let flags = DumpFlags::from_env();
    let (width, height) = flags.size.unwrap_or((1360, 820));
    let physical_width = width * ui_scale;
    let physical_height = height * ui_scale;

    let mut model = Model::new();
    model.set_mode(flags.active_mode());
    model.params.remove_bg = flags.remove_bg;
    model.params.fill_enabled = flags.fill;
    if flags.export_big && flags.cell.is_none() {
        // Enough pixels to cross the 512 MiB warning but stay below the hard
        // block, so the warning modal is exercised by the dump.
        model.params.width = 512;
        model.params.cell = 20;
    }
    if let Some(cell) = flags.cell {
        model.params.cell = cell;
    }
    if let Some(prefix) = &flags.prefix {
        model.output.prefix = Some(prefix.clone());
    }
    let mut source: Option<Arc<image::RgbaImage>> = None;
    if let Some(path) = image_path {
        let image = image::open(path)
            .with_context(|| format!("не удалось открыть {}", path.display()))?
            .into_rgba8();
        let (width, height) = image.dimensions();
        let bytes = std::fs::metadata(path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let format = image::ImageFormat::from_path(path)
            .map(|format| format!("{format:?}").to_uppercase())
            .unwrap_or_else(|_| "ИЗОБРАЖЕНИЕ".to_string());
        source = Some(Arc::new(image));
        model.on_loaded(
            0,
            path.to_path_buf(),
            Arc::clone(source.as_ref().expect("source image")),
            FileMeta {
                format,
                bytes,
                width,
                height,
            },
        );
    }

    // "ВЗЯТЬ ЦВЕТ С УГЛА" QA hook: sample the loaded image and show the toast
    // the button would produce.
    if flags.pipette {
        model.params.remove_bg = true;
        let color = model.pick_corner_color();
        if let Some(color) = color {
            model.toast(
                ToastTone::Info,
                format!(
                    "ЦВЕТ С УГЛА: #{:02X}{:02X}{:02X}",
                    color[0], color[1], color[2]
                ),
            );
        }
    }

    if let Some(image) = &source
        && flags.wants_pixel_result()
    {
        let options = model.params.preview_options();
        let conversion = pixel_image_core::convert_image(image.as_ref(), &options)
            .context("не удалось сгенерировать изображение для дампа")?;
        let params = model.params;
        let mut stats = conversion.stats;
        // The preview is converted with `cell = 1`; the export report must show
        // the real cell-scaled grid size.
        stats.output_width = stats.grid_width.saturating_mul(params.cell);
        stats.output_height = stats.grid_height.saturating_mul(params.cell);
        model.pixel_result = Some(GeneratedImage {
            sprite: Arc::new(conversion.sprite),
            stats,
            elapsed: Duration::from_millis(146),
            params,
        });
    }

    if let Some(image) = &source
        && flags.wants_cutout_result()
    {
        if flags.cutout_empty {
            let (width, height) = image.dimensions();
            let display = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 0]));
            model.cutout_result = Some(CutoutImage {
                display: Arc::new(display),
                width,
                height,
                stats: CleanStats {
                    faint_alpha_removed: 0,
                    background: Some(BackgroundStats {
                        color: [255, 255, 255],
                        removed: width as usize * height as usize,
                        removed_percent: 100.0,
                        defringe_depth: 2,
                        defringe_changed: 0,
                        thin_gaps_removed: 0,
                    }),
                },
                fully_transparent: true,
                elapsed: Duration::from_millis(146),
                params: model.params.background_params(),
            });
        } else {
            let background = model.params.background_params().options();
            let cutout = pixel_image_core::cutout_image((**image).clone(), &background)
                .context("не удалось вырезать фон для дампа")?;
            let (width, height) = cutout.image.dimensions();
            model.cutout_result = Some(CutoutImage {
                display: Arc::new(shrink_for_display(cutout.image)),
                width,
                height,
                stats: cutout.stats,
                fully_transparent: cutout.fully_transparent,
                elapsed: Duration::from_millis(146),
                params: model.params.background_params(),
            });
        }
    }

    if flags.dirty {
        match model.mode {
            Mode::PixelArt => model.params.width = 96,
            Mode::Cutout => model.params.bg_tolerance = 40,
        }
    }
    if flags.generating {
        match model.mode {
            Mode::PixelArt => {
                let params = model.params;
                model.generating = Some(GenerateJob {
                    id: 1,
                    file: model.generation,
                    params,
                });
            }
            Mode::Cutout => {
                model.cutting = Some(CutoutJob {
                    id: 1,
                    file: model.generation,
                    params: model.params.background_params(),
                });
            }
        }
    }
    let target = if flags.exporting || flags.exported {
        SaveTarget::Export
    } else {
        SaveTarget::Quick
    };
    if flags.saving || flags.exporting {
        model.saving = Some(SaveJob {
            id: 1,
            file: model.generation,
            kind: model.mode,
            target,
        });
    }
    if flags.saved || flags.exported {
        model.saving = Some(SaveJob {
            id: 1,
            file: model.generation,
            kind: model.mode,
            target,
        });
        let paths = match target {
            SaveTarget::Quick => model.save_paths(),
            SaveTarget::Export => model.export_paths(),
        };
        match model.mode {
            Mode::PixelArt => {
                let stats = model
                    .pixel_result
                    .as_ref()
                    .map(|result| result.stats.clone());
                if let (Some(stats), [grid, sprite, ..]) = (stats, paths.as_slice()) {
                    model.on_saved(
                        1,
                        model.generation,
                        grid.clone(),
                        sprite.clone(),
                        stats,
                        Duration::from_millis(146),
                    );
                }
            }
            Mode::Cutout => {
                let path = paths.into_iter().next();
                let result = model.cutout_result.as_ref().map(|result| {
                    (
                        result.width,
                        result.height,
                        result.stats.clone(),
                        result.fully_transparent,
                    )
                });
                if let (Some(path), Some((width, height, stats, fully_transparent))) =
                    (path, result)
                {
                    model.on_cutout_saved(
                        1,
                        model.generation,
                        path,
                        width,
                        height,
                        stats,
                        fully_transparent,
                        Duration::from_millis(146),
                    );
                }
            }
        }
    }
    if flags.confirm {
        model.overwrite_modal(SaveTarget::Quick);
    }
    if flags.export_confirm {
        model.overwrite_modal(SaveTarget::Export);
    }
    if flags.export_big {
        model.size_warning_modal(SaveTarget::Export);
    }
    if flags.closing {
        model.close_wait_modal();
    }
    if flags.filter_open && model.mode == Mode::PixelArt {
        model.dropdown_open = true;
    }
    if flags.edit {
        match model.mode {
            Mode::PixelArt => model.begin_edit(Field::Width),
            Mode::Cutout => model.begin_edit(Field::BgTolerance),
        }
    }
    if flags.scroll > 0 {
        model.scroll = flags.scroll;
    }

    // Undo/redo QA: start a fresh history, make one harmless output change and
    // optionally undo it, so the sidebar chips show their enabled state.
    if flags.undo_state || flags.redo_state {
        model.reset_history();
        model.output.prefix = Some("UNDO".to_string());
        model.commit_history();
        if flags.redo_state {
            model.undo();
        }
    }

    if flags.drop {
        model.drop_hover = Some(
            image_path
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("C:\\ART\\DROP.PNG")),
        );
    }

    if std::env::var_os("PIXEL_IMAGE_UI_ERROR").is_some() {
        model.on_failed(
            0,
            "НЕ УДАЛОСЬ ОТКРЫТЬ ФАЙЛ\nC:\\ART\\MISSING.PNG\nОШИБКА ФОРМАТА ИЛИ ФАЙЛ НЕ СУЩЕСТВУЕТ"
                .to_string(),
        );
    }

    if flags.export_dialog {
        model.open_export_dialog();
    }

    let mut buffer = vec![0u8; physical_width as usize * physical_height as usize * 4];
    let mut canvas = Canvas::new(&mut buffer, physical_width, physical_height, ui_scale);
    let mut ui = UiState::default();
    let font = Font::new();
    let metrics = Metrics::for_scale(ui_scale);
    view::draw(&mut canvas, &model, &mut ui, &font, metrics);

    image::save_buffer_with_format(
        output,
        &buffer,
        physical_width,
        physical_height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .with_context(|| format!("не удалось сохранить {}", output.display()))?;
    write_out(&format!("saved {}\n", output.display()));
    Ok(())
}

/// Re-attach stdout/stderr to the terminal that launched the process.
///
/// Release builds use the Windows GUI subsystem (see the crate attribute), so
/// double-clicking the executable does not open a console window. When the
/// program is started from a terminal, `--help`, `--version` and the
/// `--dump-ui` confirmation are still expected to print, so the process joins
/// the parent console. Previously redirected handles (pipes, files) are left
/// untouched. Without a parent console the call is a silent no-op.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};

    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const OPEN_EXISTING: u32 = 3;
    // `(DWORD)-11` and `(DWORD)-12`.
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4;
    const INVALID_HANDLE_VALUE: isize = -1;
    const NULL_HANDLE: isize = 0;

    // Raw kernel32 declarations keep the GUI free of extra Windows crates.
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn GetStdHandle(standard_handle: u32) -> isize;
        fn SetStdHandle(standard_handle: u32, handle: isize) -> i32;
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *const c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: isize,
        ) -> isize;
    }

    let console: Vec<u16> = std::ffi::OsStr::new("CONOUT$")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: every call uses the documented Win32 signature; the buffer is a
    // NUL-terminated UTF-16 string and handles are only stored, not dereferenced.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        let handle = CreateFileW(
            console.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        );
        if handle == INVALID_HANDLE_VALUE {
            return;
        }
        for standard_handle in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let existing = GetStdHandle(standard_handle);
            if existing == NULL_HANDLE || existing == INVALID_HANDLE_VALUE {
                SetStdHandle(standard_handle, handle);
            }
        }
    }
}

/// Best-effort console output.
///
/// Release builds use the GUI subsystem, so stdout can be a closed pipe or a
/// missing console handle; `println!` panics in that case.
fn write_out(text: &str) {
    use std::io::Write as _;
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

fn print_help() {
    let version = env!("CARGO_PKG_VERSION");
    let text = format!(
        r#"PIXEL-IMAGE GUI {version}

ИСПОЛЬЗОВАНИЕ:
    pixel-image-gui [ФАЙЛ]
    pixel-image-gui --dump-ui <OUT.png> [IMAGE]

АРГУМЕНТЫ:
    [ФАЙЛ]        изображение, открыть при запуске
    --dump-ui     отрисовать кадр UI в PNG без окна (QA)
    -h, --help    эта справка
    -V, --version версия

В ОКНЕ:
    ВКЛАДКИ       ПИКСЕЛЬ-АРТ / ВЫРЕЗАТЬ ФОН
    ОТКРЫТЬ ФАЙЛ  нативный диалог выбора изображения
    ОЧИСТИТЬ      убрать текущий файл из окна
    СГЕНЕРИРОВАТЬ построить пиксель-арт по настройкам панели
    ВЫРЕЗАТЬ      удалить фон в исходном размере (режим вырезки)
    СОХРАНИТЬ     (иконка дискеты) сохранить _grid/_sprite.png или _cut.png рядом с исходником
    ЭКСПОРТ       (иконка стрелки) окно: папка и префикс, «СОХРАНИТЬ» / «ОТМЕНА»
    ОБЗОР         в окне «ЭКСПОРТ»: выбрать папку вывода
    СБРОС         (круговая стрелка справа от «НАСТРОЙКИ») вернуть настройки и вывод к умолчаниям
    ОТМЕНА/ПОВТОР (стрелки у правого края заголовка настроек) история изменений
    ВЗЯТЬ ЦВЕТ С УГЛА  взять цвет пикселя (0,0) как ручной цвет фона
    ПЕРЕТАСКИВАНИЕ ФАЙЛА В ОКНО  открыть перетащенное изображение
    CTRL+O        открыть/заменить файл
    CTRL+L        очистить файл
    CTRL+G        выполнить действие текущего режима
    CTRL+S        быстрое сохранение рядом с исходником
    CTRL+E        окно экспорта: папка и префикс
    CTRL+МИНУС/ПЛЮС  масштаб превью; КОЛЕСО МЫШИ над превью
    CTRL+0        вписать в окно, CTRL+1 — 100%
    ПЕРЕТАСКИВАНИЕ  панорама превью при масштабе больше «по размеру»
    CTRL+Q        выход
    CTRL+Z        отменить изменение настроек
    CTRL+SHIFT+Z  повторить изменение (также CTRL+Y)
    TAB / SHIFT+TAB  следующее/предыдущее поле
    ENTER / ПРОБЕЛ  активировать поле в фокусе
    ESC           отменить ввод/закрыть диалог

ДАМПЫ СОСТОЯНИЙ (--dump-ui):
    PIXEL_IMAGE_UI_MODE=cutout    режим вырезки без результата
    PIXEL_IMAGE_UI_CUTOUT=1       режим вырезки после «ВЫРЕЗАТЬ»
    PIXEL_IMAGE_UI_CUTOUT_EMPTY=1 вырезка, полностью прозрачный результат
    PIXEL_IMAGE_UI_GENERATED=1    состояние после генерации
    PIXEL_IMAGE_UI_DIRTY=1        настройки изменены после генерации
    PIXEL_IMAGE_UI_GENERATING=1   идёт генерация/вырезка
    PIXEL_IMAGE_UI_SAVING=1       идёт сохранение
    PIXEL_IMAGE_UI_SAVED=1        уведомление «СОХРАНЕНО»
    PIXEL_IMAGE_UI_CONFIRM=1      диалог перезаписи
    PIXEL_IMAGE_UI_EXPORTING=1    идёт экспорт
    PIXEL_IMAGE_UI_EXPORTED=1     уведомление «ЭКСПОРТ ВЫПОЛНЕН»
    PIXEL_IMAGE_UI_EXPORT_CONFIRM=1 диалог перезаписи экспорта
    PIXEL_IMAGE_UI_EXPORT_DIALOG=1 окно «ЭКСПОРТ» (папка и префикс)
    PIXEL_IMAGE_UI_EXPORT_BIG=1   предупреждение о большом файле
    PIXEL_IMAGE_UI_CLOSE=1        диалог «ДОЖДАТЬСЯ И ВЫЙТИ?»
    PIXEL_IMAGE_UI_PREFIX=NAME    префикс вывода в дампе
    PIXEL_IMAGE_UI_CELL=20        размер ячейки в дампе
    PIXEL_IMAGE_UI_FILTER_OPEN=1  открытый список фильтров
    PIXEL_IMAGE_UI_EDIT=1         первое поле режима в фокусе
    PIXEL_IMAGE_UI_REMOVE_BG=1    блок удаления фона раскрыт
    PIXEL_IMAGE_UI_FILL=1         заливка фона включена
    PIXEL_IMAGE_UI_DROP=1         подсказка «ОТПУСТИТЕ ФАЙЛ» при drag&drop
    PIXEL_IMAGE_UI_PIPETTE=1      «ВЗЯТЬ ЦВЕТ С УГЛА» применена
    PIXEL_IMAGE_UI_UNDO=1         иконка «ОТМЕНА» доступна
    PIXEL_IMAGE_UI_REDO=1         иконка «ПОВТОР» доступна после отмены
    PIXEL_IMAGE_UI_SCROLL=400     прокрутка настроек, px
    PIXEL_IMAGE_UI_SIZE=1180x720  другой размер логического кадра
    PIXEL_IMAGE_UI_ERROR=1        диалог ошибки загрузки
    PIXEL_IMAGE_UI_SCALE=2        масштаб UI 200%
"#
    );
    write_out(&text);
}
