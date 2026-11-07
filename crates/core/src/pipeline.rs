//! Конвейер обработки: параметры, этапы, отчёты и сохранение файлов.
//!
//! Модуль не печатает ничего в консоль: все счётчики возвращаются в
//! [`Stats`] / [`CleanStats`], а вызывающая сторона решает, как их показать.

use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    time::Instant,
};

use image::{RgbImage, RgbaImage};
use rayon::prelude::*;

use crate::{
    clean,
    color::{Color, key},
    downscale::{Filter, apply_alpha_threshold},
    error::{Error, Result},
    grid::upscale_with_grid,
    palette::{Palette, histogram},
    parallel,
};

/// Параметры удаления однотонного фона.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackgroundOptions {
    /// Цвет фона; `None` — определить автоматически по периметру.
    pub color: Option<Color>,
    /// Допуск похожести на фон (максимум по каналам).
    pub tolerance: u8,
    /// Сколько слоёв светлой каймы снять после заливки.
    pub grow: u32,
    /// Ширина полосы матирования; `None` — подобрать по размеру изображения,
    /// `Some(0)` — выключить матирование.
    pub defringe: Option<u32>,
    /// Удалять тонкие щели цвета фона у границы.
    pub thin_gaps: bool,
    /// Удалять замкнутые просветы фона.
    pub remove_holes: bool,
    /// Минимальная площадь просвета для `remove_holes`, % площади изображения.
    pub hole_area_pct: f32,
}

impl Default for BackgroundOptions {
    fn default() -> Self {
        Self {
            color: None,
            tolerance: 12,
            grow: 0,
            defringe: None,
            thin_gaps: false,
            remove_holes: false,
            hole_area_pct: 0.04,
        }
    }
}

/// Параметры превращения изображения в пиксель-арт.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Ширина сетки в пикселях; высота считается по пропорциям.
    pub width: u32,
    /// Фильтр уменьшения.
    pub filter: Filter,
    /// Размер палитры (2..=256).
    pub colors: u16,
    /// Сторона квадрата ячейки в итоговом изображении.
    pub cell: u32,
    /// Непрозрачность линий сетки, % (0..=100).
    pub grid_opacity: u8,
    /// Цвет линий сетки.
    pub grid_color: Color,
    /// Порог бинаризации прозрачности (1..=255).
    pub alpha_threshold: u8,
    /// Залить прозрачные ячейки этим цветом (`--background`).
    pub fill: Option<Color>,
    /// Удалять изолированные группы ячеек меньше N (0 — выключено).
    pub despeckle: u32,
    /// Вырезание фона перед пикселизацией; `None` — не вырезать.
    pub background_removal: Option<BackgroundOptions>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: 128,
            filter: Filter::Lanczos,
            colors: 64,
            cell: 16,
            grid_opacity: 27,
            grid_color: [0, 0, 0],
            alpha_threshold: 128,
            fill: None,
            despeckle: 2,
            background_removal: None,
        }
    }
}

impl Options {
    /// Проверяет диапазоны, которые важны и для CLI, и для GUI.
    pub fn validate(&self) -> Result<()> {
        if self.width == 0 {
            return Err(Error::msg("ширина сетки должна быть не меньше 1"));
        }
        if !(2..=256).contains(&self.colors) {
            return Err(Error::msg("размер палитры должен быть в диапазоне 2..=256"));
        }
        if self.cell == 0 {
            return Err(Error::msg("размер ячейки должен быть не меньше 1"));
        }
        if self.grid_opacity > 100 {
            return Err(Error::msg(
                "непрозрачность сетки должна быть в диапазоне 0..=100",
            ));
        }
        if self.alpha_threshold == 0 {
            return Err(Error::msg("порог прозрачности должен быть не меньше 1"));
        }
        Ok(())
    }
}

/// Статистика вырезания фона.
#[derive(Clone, Debug, Default)]
pub struct BackgroundStats {
    /// Цвет, который считался фоном.
    pub color: Color,
    /// Сколько пикселей стало прозрачными.
    pub removed: usize,
    /// Доля удалённых пикселей, %.
    pub removed_percent: f64,
    /// Ширина полосы матирования (0 — выключено).
    pub defringe_depth: u32,
    /// Сколько пикселей изменило матирование.
    pub defringe_changed: usize,
    /// Сколько пикселей удалено как тонкие щели.
    pub thin_gaps_removed: usize,
}

/// Статистика очистки исходника (до уменьшения).
#[derive(Clone, Debug, Default)]
pub struct CleanStats {
    /// Сколько слабопрозрачных пикселей обнулено.
    pub faint_alpha_removed: usize,
    /// Данные вырезания фона, если оно выполнялось.
    pub background: Option<BackgroundStats>,
}

/// Статистика полного преобразования.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Ширина сетки.
    pub grid_width: u32,
    /// Высота сетки.
    pub grid_height: u32,
    /// Ширина файла с сеткой.
    pub output_width: u32,
    /// Высота файла с сеткой.
    pub output_height: u32,
    /// Уникальных цветов в сетке до сокращения палитры.
    pub unique_colors: usize,
    /// Итоговый размер палитры.
    pub palette_colors: usize,
    /// Прозрачных ячеек после очистки.
    pub transparent_cells: usize,
    /// Всего ячеек сетки.
    pub total_cells: usize,
    /// Сколько ячеек удалено как островки.
    pub despeckled: usize,
    /// Статистика очистки исходника.
    pub clean: CleanStats,
}

/// Тайминги этапов конвейера в миллисекундах.
///
/// Ядро остаётся молчаливым: время собирается здесь, печатает его вызывающая
/// сторона (флаг `--verbose` в CLI). Не входит в [`Stats`], потому что это
/// диагностика, а не результат преобразования.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StageTimes {
    /// Чтение файла и декодирование в RGBA (только в `*_file`-функциях).
    pub open: f32,
    /// Очистка исходника: слабая альфа, вырезание фона, матирование.
    pub clean: f32,
    /// Уменьшение до сетки.
    pub downscale: f32,
    /// Порог альфы, despeckle и подсчёт прозрачных ячеек.
    pub threshold: f32,
    /// Гистограмма уникальных цветов.
    pub histogram: f32,
    /// Построение палитры (median cut).
    pub palette: f32,
    /// Таблица ближайших цветов и ремап сетки.
    pub remap: f32,
    /// Увеличение и наложение сетки.
    pub upscale: f32,
    /// Запись PNG на диск (только в `*_file`-функциях).
    pub save: f32,
}

impl StageTimes {
    /// Сумма этапов, мс.
    pub fn total(&self) -> f32 {
        self.open
            + self.clean
            + self.downscale
            + self.threshold
            + self.histogram
            + self.palette
            + self.remap
            + self.upscale
            + self.save
    }
}

/// Миллисекунды с момента `started`.
fn since(started: Instant) -> f32 {
    started.elapsed().as_secs_f32() * 1000.0
}

/// Результат преобразования в памяти.
#[derive(Debug)]
pub struct Conversion {
    /// Чистый спрайт в разрешении сетки.
    ///
    /// Растр с сеткой здесь намеренно отсутствует: превью рисует сетку на лету,
    /// а экспорт строит её [`render_grid`] уже на диске.
    pub sprite: RgbaImage,
    /// Счётчики этапов.
    pub stats: Stats,
}

/// Результат преобразования с сохранением на диск.
#[derive(Debug)]
pub struct SavedConversion {
    /// Путь к файлу с сеткой.
    pub grid_path: PathBuf,
    /// Путь к файлу спрайта.
    pub sprite_path: PathBuf,
    /// Счётчики этапов.
    pub stats: Stats,
    /// Время по этапам, включая чтение файла и запись PNG.
    pub times: StageTimes,
}

/// Результат вырезания фона в памяти.
#[derive(Debug)]
pub struct Cutout {
    /// Вырезанное изображение в исходном размере.
    pub image: RgbaImage,
    /// Статистика очистки.
    pub stats: CleanStats,
    /// Правда ли, что после вырезания не осталось непрозрачных пикселей.
    pub fully_transparent: bool,
}

/// Результат вырезания фона с сохранением на диск.
#[derive(Debug)]
pub struct SavedCutout {
    /// Путь к `_cut.png`.
    pub path: PathBuf,
    /// Ширина результата.
    pub width: u32,
    /// Высота результата.
    pub height: u32,
    /// Статистика очистки.
    pub stats: CleanStats,
    /// Правда ли, что результат полностью прозрачный.
    pub fully_transparent: bool,
    /// Время по этапам: чтение файла, очистка, запись PNG.
    pub times: StageTimes,
}

/// Очистка исходника: слабая альфа, вырезание фона, тонкие щели, матирование.
///
/// Меняет `src` на месте и возвращает счётчики. Если `background` = `None`,
/// выполняется только отсечение слабой альфы.
///
/// Одноразовый вариант: создаёт свои буферы. Хосту с несколькими генерациями
/// подряд (GUI-воркер) выгоднее [`clean_source_with_scratch`].
pub fn clean_source(src: &mut RgbaImage, background: Option<&BackgroundOptions>) -> CleanStats {
    clean_source_with_scratch(src, background, &mut clean::Scratch::default())
}

/// То же, что [`clean_source`], с переиспользуемыми буферами очистки.
pub fn clean_source_with_scratch(
    src: &mut RgbaImage,
    background: Option<&BackgroundOptions>,
    scratch: &mut clean::Scratch,
) -> CleanStats {
    let (w, h) = src.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let buf = src.as_mut();
    let faint_alpha_removed = cut_faint_alpha_blocked(buf, 16);

    let Some(bg_opts) = background else {
        return CleanStats {
            faint_alpha_removed,
            background: None,
        };
    };

    let bg = bg_opts
        .color
        .unwrap_or_else(|| clean::detect_background(buf, wu, hu));
    let min_hole = if bg_opts.remove_holes {
        ((wu * hu) as f64 * bg_opts.hole_area_pct as f64 / 100.0)
            .round()
            .max(1.0) as usize
    } else {
        0
    };
    let removed = clean::remove_background_in(
        buf,
        wu,
        hu,
        bg,
        bg_opts.tolerance,
        bg_opts.grow,
        min_hole,
        scratch,
    );

    let depth = bg_opts
        .defringe
        .unwrap_or_else(|| clean::auto_defringe_depth(wu, hu));
    let reach = if depth > 0 {
        depth
    } else {
        clean::auto_defringe_depth(wu, hu)
    };
    let thin_gaps_removed = if bg_opts.thin_gaps {
        clean::remove_thin_pockets_in(buf, wu, hu, bg, bg_opts.tolerance, reach, 2, scratch)
    } else {
        0
    };
    let defringe_changed = clean::defringe_in(buf, wu, hu, bg, depth, scratch);

    CleanStats {
        faint_alpha_removed,
        background: Some(BackgroundStats {
            color: bg,
            removed,
            removed_percent: 100.0 * removed as f64 / (wu * hu) as f64,
            defringe_depth: depth,
            defringe_changed,
            thin_gaps_removed,
        }),
    }
}

/// Пиксели со слабой альфой (мягкая маска от JPEG-сглаживания) считаются
/// полностью прозрачными. Раньше для этого исходник копировали и чистили на
/// месте; теперь вырезание делает уменьшение (см. `Filter::downscale`).
pub const FAINT_ALPHA: u8 = 16;

/// Сколько байт в одном куске при параллельном отсечении слабой альфы.
/// Кратно 4, чтобы кусок не разрезал пиксель.
const FAINT_BLOCK: usize = 16 * 1024;

/// Отсечение слабой альфы по крупным кускам.
///
/// Арифметика — ровно [`clean::cut_faint_alpha`], куски независимы, поэтому
/// результат не зависит от их числа и порядка. Мелкие буферы (тестовые
/// фикстуры 256×256) остаются на последовательном пути: параллель им не
/// помогает, а тесты бюджета аллокаций не должны видеть чужой пул.
fn cut_faint_alpha_blocked(rgba: &mut [u8], cut: u8) -> usize {
    if rgba.len() <= FAINT_BLOCK || !parallel::should_parallel_bytes(rgba.len()) {
        return clean::cut_faint_alpha(rgba, cut);
    }
    rgba.par_chunks_mut(FAINT_BLOCK)
        .map(|block| clean::cut_faint_alpha(block, cut))
        .sum()
}

/// Сумма `pred` по строкам шириной `per_row`; параллельно на больших объёмах.
fn count_rows<T: Sync>(items: &[T], per_row: usize, pred: impl Fn(&T) -> bool + Sync) -> usize {
    let count = |row: &[T]| row.iter().filter(|x| pred(x)).count();
    if per_row == 0 || !parallel::should_parallel(items.len()) {
        return count(items);
    }
    items.par_chunks(per_row).map(count).sum()
}

/// Сколько пикселей со слабой альфой есть в исходнике — для [`CleanStats`].
fn count_faint_alpha(src: &RgbaImage) -> usize {
    let pixels = src.as_raw().as_chunks::<4>().0;
    count_rows(pixels, src.width() as usize, |p| {
        p[3] != 0 && p[3] < FAINT_ALPHA
    })
}

/// Сколько ячеек полностью прозрачны, для [`Stats`].
fn count_transparent(img: &RgbaImage) -> usize {
    let pixels = img.as_raw().as_chunks::<4>().0;
    count_rows(pixels, img.width() as usize, |p| p[3] == 0)
}

/// Замена цветов строки по таблице ближайших цветов палитры.
fn remap_row(row: &mut [u8], remap: &HashMap<u32, Color>) {
    for p in row.as_chunks_mut::<4>().0 {
        if p[3] != 0 {
            let c = remap[&key([p[0], p[1], p[2]])];
            p[..3].copy_from_slice(&c);
        }
    }
}

/// Превращает RGBA-изображение в пиксель-арт, не трогая диск.
///
/// Исходник только читается: копия делается лишь при включённом вырезании фона
/// (там изображение действительно меняется). Растр сетки здесь не строится —
/// для превью он не нужен, для экспорта есть [`render_grid`].
pub fn convert_image(src: &RgbaImage, options: &Options) -> Result<Conversion> {
    convert_image_timed(src, options).map(|(conversion, _)| conversion)
}

/// То же, что [`convert_image`], с переиспользуемыми буферами очистки.
pub fn convert_image_with_scratch(
    src: &RgbaImage,
    options: &Options,
    scratch: &mut clean::Scratch,
) -> Result<Conversion> {
    convert_image_timed_scratch(src, options, scratch).map(|(conversion, _)| conversion)
}

/// То же, что [`convert_image`], но возвращает и время по этапам.
///
/// Нужно для `--verbose` в CLI: ядро само ничего не печатает.
pub fn convert_image_timed(
    src: &RgbaImage,
    options: &Options,
) -> Result<(Conversion, StageTimes)> {
    convert_image_timed_scratch(src, options, &mut clean::Scratch::default())
}

/// [`convert_image_timed`] с переиспользуемыми буферами очистки.
pub fn convert_image_timed_scratch(
    src: &RgbaImage,
    options: &Options,
    scratch: &mut clean::Scratch,
) -> Result<(Conversion, StageTimes)> {
    options.validate()?;
    let mut times = StageTimes::default();

    let started = Instant::now();
    // Копия нужна только вырезанию фона. В обычном режиме работаем по ссылке,
    // а слабую альфу отсекает само уменьшение.
    let (clean_stats, working) = match options.background_removal.as_ref() {
        Some(background) => {
            let mut copy = src.clone();
            let stats = clean_source_with_scratch(&mut copy, Some(background), scratch);
            (stats, Cow::Owned(copy))
        }
        None => (
            CleanStats {
                faint_alpha_removed: count_faint_alpha(src),
                background: None,
            },
            Cow::Borrowed(src),
        ),
    };
    times.clean = since(started);

    let (w, h) = working.dimensions();
    if w == 0 || h == 0 {
        return Err(Error::msg("пустое изображение"));
    }

    let grid_w = options.width;
    let grid_h = ((h as f64 * grid_w as f64 / w as f64).round() as u32).max(1);

    let started = Instant::now();
    let mut sprite = options
        .filter
        .downscale(&working, grid_w, grid_h, FAINT_ALPHA);
    times.downscale = since(started);
    drop(working); // исходник больше не нужен

    let started = Instant::now();
    apply_alpha_threshold(&mut sprite, options.alpha_threshold);
    let despeckled = clean::despeckle(
        sprite.as_mut(),
        grid_w as usize,
        grid_h as usize,
        options.despeckle as usize,
    );
    let transparent_cells = count_transparent(&sprite);
    times.threshold = since(started);

    let started = Instant::now();
    let hist = histogram(&sprite);
    times.histogram = since(started);
    if hist.is_empty() {
        return Err(Error::msg("изображение полностью прозрачное"));
    }
    let unique_colors = hist.len();

    let started = Instant::now();
    let palette = Palette::median_cut(hist.clone(), options.colors as usize);
    times.palette = since(started);

    let started = Instant::now();
    let remap: HashMap<u32, Color> = hist
        .iter()
        .map(|e| (key(e.color), palette.nearest(e.color)))
        .collect();

    let width = sprite.width() as usize;
    let sprite_raw = sprite.as_mut();
    if width > 0 && parallel::should_parallel(sprite_raw.len() / 4) {
        // Каждый поток правит свои строки; таблица `remap` общая и только читается.
        sprite_raw
            .par_chunks_mut(width * 4)
            .for_each(|row| remap_row(row, &remap));
    } else {
        remap_row(sprite_raw, &remap);
    }
    times.remap = since(started);

    // Растр сетки здесь не строится: превью рисует сетку само (GUI), а экспорт
    // вызывает `render_grid` из `convert_file`. Размер выхода считается
    // арифметикой, чтобы статистика осталась прежней.
    let output_width = grid_w
        .checked_mul(options.cell)
        .ok_or_else(|| Error::msg("итоговая ширина слишком велика"))?;
    let output_height = grid_h
        .checked_mul(options.cell)
        .ok_or_else(|| Error::msg("итоговая высота слишком велика"))?;

    let stats = Stats {
        grid_width: grid_w,
        grid_height: grid_h,
        output_width,
        output_height,
        unique_colors,
        palette_colors: palette.len(),
        transparent_cells,
        total_cells: grid_w as usize * grid_h as usize,
        despeckled,
        clean: clean_stats,
    };

    Ok((Conversion { sprite, stats }, times))
}

/// Строит итоговое изображение с сеткой из готового спрайта.
///
/// Нужно только экспорту: превью в GUI рисует сетку на лету и этот растр не
/// использует. Вынесено отдельно, чтобы `convert_image` не платил за
/// `grid_w·cell × grid_h·cell × 4` байт на каждую генерацию.
pub fn render_grid(sprite: &RgbaImage, options: &Options) -> Result<RgbaImage> {
    let alpha = (options.grid_opacity as u32 * 255 + 50) / 100;
    if let Some(fill) = options.fill {
        let mut filled = sprite.clone();
        for p in filled.as_mut().as_chunks_mut::<4>().0 {
            if p[3] == 0 {
                p.copy_from_slice(&[fill[0], fill[1], fill[2], 255]);
            }
        }
        upscale_with_grid(&filled, options.cell, options.grid_color, alpha, true)
    } else {
        upscale_with_grid(sprite, options.cell, options.grid_color, alpha, true)
    }
}

/// Читает файл, превращает его в пиксель-арт и сохраняет `_grid.png` и
/// `_sprite.png` рядом с исходником (если в `prefix` нет другого пути).
pub fn convert_file(
    input: &Path,
    options: &Options,
    prefix: Option<&str>,
) -> Result<SavedConversion> {
    let started = Instant::now();
    let src = image::open(input)?.into_rgba8();
    let mut times = StageTimes {
        open: since(started),
        ..StageTimes::default()
    };

    let (Conversion { sprite, stats }, stage) = convert_image_timed(&src, options)?;
    times.clean = stage.clean;
    times.downscale = stage.downscale;
    times.threshold = stage.threshold;
    times.histogram = stage.histogram;
    times.palette = stage.palette;
    times.remap = stage.remap;
    drop(src); // исходник для сетки не нужен

    let started = Instant::now();
    let grid = render_grid(&sprite, options)?;
    times.upscale = since(started);

    let started = Instant::now();
    let grid_path = output_path(input, prefix, "_grid.png");
    let sprite_path = output_path(input, prefix, "_sprite.png");
    save_png(&grid, options.fill, &grid_path)?;
    save_png(&sprite, options.fill, &sprite_path)?;
    times.save = since(started);

    Ok(SavedConversion {
        grid_path,
        sprite_path,
        stats,
        times,
    })
}

/// Вырезает фон из RGBA-изображения, не трогая диск.
pub fn cutout_image(src: RgbaImage, background: &BackgroundOptions) -> Result<Cutout> {
    cutout_image_with_scratch(src, background, &mut clean::Scratch::default())
}

/// То же, что [`cutout_image`], с переиспользуемыми буферами очистки.
pub fn cutout_image_with_scratch(
    mut src: RgbaImage,
    background: &BackgroundOptions,
    scratch: &mut clean::Scratch,
) -> Result<Cutout> {
    let stats = clean_source_with_scratch(&mut src, Some(background), scratch);
    let fully_transparent = count_transparent(&src) == src.width() as usize * src.height() as usize;
    Ok(Cutout {
        image: src,
        stats,
        fully_transparent,
    })
}

/// Читает файл, вырезает фон и сохраняет `<prefix>_cut.png` в исходном размере.
pub fn cutout_file(
    input: &Path,
    background: &BackgroundOptions,
    prefix: Option<&str>,
) -> Result<SavedCutout> {
    let started = Instant::now();
    let src = image::open(input)?.into_rgba8();
    let open = since(started);

    let started = Instant::now();
    let cut = cutout_image(src, background)?;
    let clean = since(started);

    let path = output_path(input, prefix, "_cut.png");
    let (width, height) = cut.image.dimensions();
    let started = Instant::now();
    save_png(&cut.image, None, &path)?;
    let save = since(started);
    Ok(SavedCutout {
        path,
        width,
        height,
        stats: cut.stats,
        fully_transparent: cut.fully_transparent,
        times: StageTimes {
            open,
            clean,
            save,
            ..StageTimes::default()
        },
    })
}

/// Путь к выходному файлу: папка входного файла + префикс + суффикс.
///
/// Если `prefix` не задан, берётся имя входного файла без расширения. Путь из
/// `prefix` используется как есть (в том числе абсолютный).
pub fn output_path(input: &Path, prefix: Option<&str>, suffix: &str) -> PathBuf {
    let prefix = prefix.map(str::to_owned).unwrap_or_else(|| {
        input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "pixel".into())
    });
    let dir = input
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf();
    dir.join(format!("{prefix}{suffix}"))
}

/// Сохраняет PNG: с заливкой фона, если задан `fill`; в RGB, если прозрачных
/// пикселей нет; иначе в RGBA с сохранением прозрачности.
///
/// Композит и упаковка идут по строкам: строка не зависит от соседних, поэтому
/// строки можно раздать потокам. Само сжатие PNG остаётся однопоточным —
/// порядок блоков IDAT фиксирован.
pub fn save_png(img: &RgbaImage, fill: Option<Color>, path: &Path) -> Result<()> {
    let (w, h) = img.dimensions();
    let raw = img.as_raw();
    let pixels = raw.as_chunks::<4>().0;
    let width = (w as usize).max(1);
    let parallel = parallel::should_parallel(pixels.len());

    if let Some(bg) = fill {
        let mut out = vec![0u8; w as usize * h as usize * 3];
        let row_bytes = (width * 3).max(1);
        let composite = |src: &[[u8; 4]], dst: &mut [u8]| {
            for (dst, p) in dst.chunks_exact_mut(3).zip(src) {
                let a = p[3] as u32;
                for c in 0..3 {
                    dst[c] = ((p[c] as u32 * a + bg[c] as u32 * (255 - a) + 127) / 255) as u8;
                }
            }
        };
        if parallel {
            pixels
                .par_chunks(width)
                .zip(out.par_chunks_mut(row_bytes))
                .for_each(|(src, dst)| composite(src, dst));
        } else {
            for (src, dst) in pixels.chunks(width).zip(out.chunks_mut(row_bytes)) {
                composite(src, dst);
            }
        }
        RgbImage::from_raw(w, h, out)
            .ok_or_else(|| Error::msg("размер буфера"))?
            .save(path)?;
    } else {
        let opaque = |row: &[[u8; 4]]| row.iter().all(|p| p[3] == 255);
        let all_opaque = if parallel {
            pixels.par_chunks(width).all(opaque)
        } else {
            pixels.chunks(width).all(opaque)
        };
        if all_opaque {
            let mut out = vec![0u8; w as usize * h as usize * 3];
            let row_bytes = (width * 3).max(1);
            let pack = |src: &[[u8; 4]], dst: &mut [u8]| {
                for (dst, p) in dst.chunks_exact_mut(3).zip(src) {
                    dst.copy_from_slice(&p[..3]);
                }
            };
            if parallel {
                pixels
                    .par_chunks(width)
                    .zip(out.par_chunks_mut(row_bytes))
                    .for_each(|(src, dst)| pack(src, dst));
            } else {
                for (src, dst) in pixels.chunks(width).zip(out.chunks_mut(row_bytes)) {
                    pack(src, dst);
                }
            }
            RgbImage::from_raw(w, h, out)
                .ok_or_else(|| Error::msg("размер буфера"))?
                .save(path)?;
        } else {
            img.save(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_path(name: &str) -> PathBuf {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pixel-image-test-{}-{seq}-{name}.png",
            std::process::id()
        ))
    }

    /// Цветовой тип PNG из байтов IHDR: 2 = RGB, 6 = RGBA.
    fn png_color_type(path: &Path) -> u8 {
        let bytes = std::fs::read(path).expect("файл записан");
        // сигнатура (8) + длина (4) + "IHDR" (4) + width (4) + height (4) + bit depth (1)
        bytes[25]
    }

    fn read_back(path: &Path) -> RgbaImage {
        image::open(path)
            .expect("обратное чтение PNG")
            .into_rgba8()
    }

    #[test]
    fn save_png_opaque_writes_rgb() {
        let img = RgbaImage::from_raw(
            3,
            1,
            vec![10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255],
        )
        .expect("фикстура");
        let path = temp_path("opaque");

        save_png(&img, None, &path).expect("запись");
        assert_eq!(png_color_type(&path), 2, "непрозрачная картинка — RGB");
        assert_eq!(read_back(&path).as_raw(), img.as_raw());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_png_transparent_writes_rgba() {
        let img =
            RgbaImage::from_raw(2, 1, vec![10, 20, 30, 255, 40, 50, 60, 0]).expect("фикстура");
        let path = temp_path("rgba");

        save_png(&img, None, &path).expect("запись");
        assert_eq!(png_color_type(&path), 6, "есть прозрачность — RGBA");
        assert_eq!(read_back(&path).as_raw(), img.as_raw());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_png_fill_composites_over_background() {
        let img = RgbaImage::from_raw(
            3,
            1,
            vec![10, 20, 30, 128, 40, 50, 60, 0, 70, 80, 90, 255],
        )
        .expect("фикстура");
        let bg: Color = [255, 255, 255];
        let path = temp_path("fill");

        save_png(&img, Some(bg), &path).expect("запись");
        assert_eq!(png_color_type(&path), 2, "с заливкой — RGB");

        let got = read_back(&path);
        let raw = img.as_raw();
        for (i, p) in raw.as_chunks::<4>().0.iter().enumerate() {
            let a = p[3] as u32;
            let want: Vec<u8> = (0..3)
                .map(|c| ((p[c] as u32 * a + bg[c] as u32 * (255 - a) + 127) / 255) as u8)
                .collect();
            let px = got.get_pixel(i as u32, 0).0;
            assert_eq!(&px[..3], want.as_slice(), "пиксель {i}");
            assert_eq!(px[3], 255, "альфа после заливки {i}");
        }

        std::fs::remove_file(&path).ok();
    }

    /// Фикстура «фото»: полупрозрачная кайма, слабая альфа и прозрачные пиксели.
    fn photo(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let alpha = match (x + y) % 11 {
                    0 => 0,
                    1 => 12, // слабая альфа — должен считаться прозрачным
                    2 => 140,
                    _ => 255,
                };
                img.put_pixel(
                    x,
                    y,
                    image::Rgba([(x * 9 % 256) as u8, (y * 7 % 256) as u8, 200, alpha]),
                );
            }
        }
        img
    }

    /// Превью не материализует сетку: размеры выхода считаются арифметикой,
    /// а растр появляется только в `render_grid`.
    #[test]
    fn convert_image_needs_no_grid() {
        let src = photo(16, 12);
        let options = Options {
            width: 8,
            cell: 5,
            colors: 8,
            ..Options::default()
        };

        let conversion = convert_image(&src, &options).expect("преобразование");
        assert_eq!(conversion.sprite.dimensions(), (8, 6));
        assert_eq!(conversion.stats.output_width, 40);
        assert_eq!(conversion.stats.output_height, 30);

        let grid = render_grid(&conversion.sprite, &options).expect("сетка");
        assert_eq!(grid.dimensions(), (40, 30));
    }

    /// Исходник не мутируется ни в обычном режиме, ни при вырезании фона.
    #[test]
    fn source_not_mutated() {
        let src = photo(24, 18);
        let before: Vec<u8> = src.as_raw().clone();

        let plain = Options {
            width: 12,
            cell: 4,
            ..Options::default()
        };
        convert_image(&src, &plain).expect("без вырезания фона");
        assert_eq!(src.as_raw(), &before, "обычный режим не меняет исходник");

        let with_bg = Options {
            width: 12,
            cell: 4,
            background_removal: Some(BackgroundOptions::default()),
            ..Options::default()
        };
        convert_image(&src, &with_bg).expect("с вырезанием фона");
        assert_eq!(src.as_raw(), &before, "вырезание фона не меняет исходник");
    }

    /// Отсечение слабой альфы внутри уменьшения эквивалентно прежней
    /// предварительной зачистке копии.
    #[test]
    fn faint_alpha_equivalent_to_precut() {
        let src = photo(40, 30);
        let options = Options {
            width: 20,
            cell: 4,
            ..Options::default()
        };

        let mut cut = src.clone();
        let removed = clean::cut_faint_alpha(cut.as_mut(), FAINT_ALPHA);
        assert!(removed > 0, "фикстура обязана содержать слабую альфу");

        let direct = convert_image(&src, &options).expect("как есть");
        let precut = convert_image(&cut, &options).expect("предварительно очищенный");

        assert_eq!(
            direct.sprite.as_raw(),
            precut.sprite.as_raw(),
            "спрайт обязан совпасть"
        );
        assert_eq!(direct.stats.unique_colors, precut.stats.unique_colors);
        assert_eq!(direct.stats.palette_colors, precut.stats.palette_colors);
        assert_eq!(
            direct.stats.clean.faint_alpha_removed, removed,
            "счётчик слабой альфы считается по исходнику"
        );
    }
}
