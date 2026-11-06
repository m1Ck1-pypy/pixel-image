//! Конвейер обработки: параметры, этапы, отчёты и сохранение файлов.
//!
//! Модуль не печатает ничего в консоль: все счётчики возвращаются в
//! [`Stats`] / [`CleanStats`], а вызывающая сторона решает, как их показать.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use image::{RgbImage, RgbaImage};

use crate::{
    clean,
    color::{Color, key},
    downscale::{Filter, apply_alpha_threshold},
    error::{Error, Result},
    grid::upscale_with_grid,
    palette::{Palette, histogram},
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

/// Результат преобразования в памяти.
#[derive(Debug)]
pub struct Conversion {
    /// Пиксель-арт с сеткой.
    pub grid: RgbaImage,
    /// Чистый спрайт в разрешении сетки.
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
}

/// Очистка исходника: слабая альфа, вырезание фона, тонкие щели, матирование.
///
/// Меняет `src` на месте и возвращает счётчики. Если `background` = `None`,
/// выполняется только отсечение слабой альфы.
pub fn clean_source(src: &mut RgbaImage, background: Option<&BackgroundOptions>) -> CleanStats {
    let (w, h) = src.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let buf = src.as_mut();
    let faint_alpha_removed = clean::cut_faint_alpha(buf, 16);

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
    let removed =
        clean::remove_background(buf, wu, hu, bg, bg_opts.tolerance, bg_opts.grow, min_hole);

    let depth = bg_opts
        .defringe
        .unwrap_or_else(|| clean::auto_defringe_depth(wu, hu));
    let reach = if depth > 0 {
        depth
    } else {
        clean::auto_defringe_depth(wu, hu)
    };
    let thin_gaps_removed = if bg_opts.thin_gaps {
        clean::remove_thin_pockets(buf, wu, hu, bg, bg_opts.tolerance, reach, 2)
    } else {
        0
    };
    let defringe_changed = clean::defringe(buf, wu, hu, bg, depth);

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

/// Превращает RGBA-изображение в пиксель-арт, не трогая диск.
pub fn convert_image(mut src: RgbaImage, options: &Options) -> Result<Conversion> {
    options.validate()?;
    let clean_stats = clean_source(&mut src, options.background_removal.as_ref());

    let (w, h) = src.dimensions();
    if w == 0 || h == 0 {
        return Err(Error::msg("пустое изображение"));
    }

    let grid_w = options.width;
    let grid_h = ((h as f64 * grid_w as f64 / w as f64).round() as u32).max(1);
    let mut sprite = options.filter.downscale(&src, grid_w, grid_h);
    drop(src); // исходник больше не нужен

    apply_alpha_threshold(&mut sprite, options.alpha_threshold);
    let despeckled = clean::despeckle(
        sprite.as_mut(),
        grid_w as usize,
        grid_h as usize,
        options.despeckle as usize,
    );
    let transparent_cells = sprite
        .as_raw()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] == 0)
        .count();

    let hist = histogram(&sprite);
    if hist.is_empty() {
        return Err(Error::msg("изображение полностью прозрачное"));
    }
    let unique_colors = hist.len();
    let palette = Palette::median_cut(hist.clone(), options.colors as usize);
    let remap: HashMap<u32, Color> = hist
        .iter()
        .map(|e| (key(e.color), palette.nearest(e.color)))
        .collect();

    for p in sprite.as_mut().as_chunks_mut::<4>().0 {
        if p[3] != 0 {
            let c = remap[&key([p[0], p[1], p[2]])];
            p[..3].copy_from_slice(&c);
        }
    }

    let alpha = (options.grid_opacity as u32 * 255 + 50) / 100;
    let big = if let Some(fill) = options.fill {
        let mut filled = sprite.clone();
        for p in filled.as_mut().as_chunks_mut::<4>().0 {
            if p[3] == 0 {
                p.copy_from_slice(&[fill[0], fill[1], fill[2], 255]);
            }
        }
        upscale_with_grid(&filled, options.cell, options.grid_color, alpha, true)?
    } else {
        upscale_with_grid(&sprite, options.cell, options.grid_color, alpha, true)?
    };

    let output_width = big.width();
    let output_height = big.height();
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

    Ok(Conversion {
        grid: big,
        sprite,
        stats,
    })
}

/// Читает файл, превращает его в пиксель-арт и сохраняет `_grid.png` и
/// `_sprite.png` рядом с исходником (если в `prefix` нет другого пути).
pub fn convert_file(
    input: &Path,
    options: &Options,
    prefix: Option<&str>,
) -> Result<SavedConversion> {
    let src = image::open(input)?.into_rgba8();
    let Conversion {
        grid,
        sprite,
        stats,
    } = convert_image(src, options)?;

    let grid_path = output_path(input, prefix, "_grid.png");
    let sprite_path = output_path(input, prefix, "_sprite.png");
    save_png(&grid, options.fill, &grid_path)?;
    save_png(&sprite, options.fill, &sprite_path)?;

    Ok(SavedConversion {
        grid_path,
        sprite_path,
        stats,
    })
}

/// Вырезает фон из RGBA-изображения, не трогая диск.
pub fn cutout_image(mut src: RgbaImage, background: &BackgroundOptions) -> Result<Cutout> {
    let stats = clean_source(&mut src, Some(background));
    let fully_transparent = src.as_raw().as_chunks::<4>().0.iter().all(|p| p[3] == 0);
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
    let src = image::open(input)?.into_rgba8();
    let cut = cutout_image(src, background)?;
    let path = output_path(input, prefix, "_cut.png");
    let (width, height) = cut.image.dimensions();
    save_png(&cut.image, None, &path)?;
    Ok(SavedCutout {
        path,
        width,
        height,
        stats: cut.stats,
        fully_transparent: cut.fully_transparent,
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
pub fn save_png(img: &RgbaImage, fill: Option<Color>, path: &Path) -> Result<()> {
    let (w, h) = img.dimensions();
    let raw = img.as_raw();
    if let Some(bg) = fill {
        let mut out = Vec::with_capacity(w as usize * h as usize * 3);
        for p in raw.as_chunks::<4>().0 {
            let a = p[3] as u32;
            for c in 0..3 {
                out.push(((p[c] as u32 * a + bg[c] as u32 * (255 - a) + 127) / 255) as u8);
            }
        }
        RgbImage::from_raw(w, h, out)
            .ok_or_else(|| Error::msg("размер буфера"))?
            .save(path)?;
    } else if raw.as_chunks::<4>().0.iter().all(|p| p[3] == 255) {
        let out: Vec<u8> = raw
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        RgbImage::from_raw(w, h, out)
            .ok_or_else(|| Error::msg("размер буфера"))?
            .save(path)?;
    } else {
        img.save(path)?;
    }
    Ok(())
}
