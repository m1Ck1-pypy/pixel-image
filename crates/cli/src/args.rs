use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use pixel_image_core::{BackgroundOptions, Color, Filter, Options, color::parse_hex};

/// Способ уменьшения изображения до сетки.
///
/// Дублирует [`pixel_image_core::Filter`], чтобы ядро не зависело от clap.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum FilterArg {
    /// Lanczos3: самый чёткий, но возможны ореолы у резких границ
    Lanczos,
    /// Catmull-Rom: чёткий, ореолы слабее, чем у Lanczos
    Catmull,
    /// Билинейный: мягкий, без ореолов
    Triangle,
    /// Гауссов: самый размытый
    Gaussian,
    /// Ближайший сосед: берётся один пиксель, самые жёсткие края, возможны потери деталей
    Nearest,
    /// Среднее цветов блока: честное усреднение, без ореолов
    Box,
    /// Самый частый цвет блока: чистые края, без смешанных оттенков
    Mode,
}

impl From<FilterArg> for Filter {
    fn from(filter: FilterArg) -> Self {
        match filter {
            FilterArg::Lanczos => Filter::Lanczos,
            FilterArg::Catmull => Filter::Catmull,
            FilterArg::Triangle => Filter::Triangle,
            FilterArg::Gaussian => Filter::Gaussian,
            FilterArg::Nearest => Filter::Nearest,
            FilterArg::Box => Filter::Box,
            FilterArg::Mode => Filter::Mode,
        }
    }
}

/// Превращает изображение в пиксель-арт с чёткой сеткой.
///
/// Создаёт два PNG в папке входного файла: <prefix>_grid.png (с сеткой)
/// и <prefix>_sprite.png (чистый спрайт).
///
/// В режиме --cutbg вместо пиксель-арта создаётся один <prefix>_cut.png:
/// исходное изображение в исходном размере, но с прозрачным фоном.
#[derive(Parser, Debug)]
#[command(
    name = "pixelgrid",
    version,
    about = "Превращает изображение в пиксель-арт с чёткой сеткой",
    long_about = None
)]
pub struct Args {
    /// Входное изображение (JPEG, PNG, WebP, BMP, GIF)
    pub input: PathBuf,

    /// Ширина сетки в пикселях (высота считается по пропорциям)
    #[arg(
        short,
        long,
        default_value_t = 128,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub width: u32,

    /// Фильтр уменьшения
    #[arg(short = 'f', long, value_enum, default_value_t = FilterArg::Lanczos)]
    pub filter: FilterArg,

    /// Размер палитры (2..=256)
    #[arg(
        short,
        long,
        default_value_t = 64,
        value_parser = clap::value_parser!(u16).range(2..=256)
    )]
    pub colors: u16,

    /// Размер одной ячейки в итоговом файле, px (>= 2)
    #[arg(
        short = 's',
        long,
        default_value_t = 16,
        value_parser = clap::value_parser!(u32).range(2..)
    )]
    pub cell: u32,

    /// Непрозрачность линий сетки, % (0..=100)
    #[arg(
        short = 'o',
        long = "grid-opacity",
        default_value_t = 27,
        value_parser = clap::value_parser!(u8).range(0..=100)
    )]
    pub grid_opacity: u8,

    /// Цвет линий сетки в HEX (RRGGBB, можно с #)
    #[arg(
        short = 'g',
        long = "grid-color",
        value_name = "HEX",
        default_value = "000000",
        value_parser = parse_hex
    )]
    pub grid_color: Color,

    /// Порог прозрачности (1..=255): ячейки с альфой ниже порога станут прозрачными
    #[arg(
        long,
        default_value_t = 128,
        value_parser = clap::value_parser!(u8).range(1..)
    )]
    pub alpha_threshold: u8,

    /// Залить прозрачные области цветом HEX (по умолчанию прозрачность сохраняется)
    #[arg(short = 'b', long, value_name = "HEX", value_parser = parse_hex)]
    pub background: Option<Color>,

    /// Вырезать однотонный фон заливкой от краёв изображения (нужно для JPG и
    /// PNG без прозрачности)
    #[arg(long)]
    pub remove_bg: bool,

    /// Отдельный режим: только вырезать фон и сохранить изображение в исходном
    /// размере с прозрачным фоном (<prefix>_cut.png). Пиксель-арт не создаётся;
    /// пиксельные опции (--width, --filter, --colors, --cell, --grid-opacity,
    /// --grid-color, --alpha-threshold, --background, --despeckle) с этим режимом
    /// несовместимы. Флаг --remove-bg подразумевается
    #[arg(long)]
    pub cutbg: bool,

    /// Цвет фона для удаления фона в HEX (по умолчанию определяется по краям).
    /// Требует --remove-bg или --cutbg
    #[arg(long, value_name = "HEX", value_parser = parse_hex)]
    pub bg_color: Option<Color>,

    /// Допуск цвета фона для удаления фона (0..=255): насколько цвет может
    /// отличаться от фона, чтобы считаться фоном (шум JPEG, антиалиасинг)
    #[arg(long, default_value_t = 12)]
    pub bg_tolerance: u8,

    /// Сколько слоёв светлой каймы снять вдоль границы после вырезания фона
    /// (0 — не снимать; 1–2 убирают белый ореол, но могут съесть светлый контур)
    #[arg(long, default_value_t = 0)]
    pub bg_grow: u32,

    /// Ширина полосы (в пикселях исходника) у границы, где убирается светлая кайма
    /// от фона: цвет очищается от примеси фона, прозрачность выставляется по доле
    /// объекта. По умолчанию включено и подбирается по размеру изображения;
    /// 0 — выключить. Для больших -w (256 и выше) без этого края остаются светлыми
    #[arg(long, value_name = "PX")]
    pub bg_defringe: Option<u32>,

    /// Удалять тонкие щели цвета фона у границы (между прядью и телом). Может
    /// съесть тонкие светлые детали белых объектов (обувь), поэтому по умолчанию выключено
    #[arg(long)]
    pub thin_gaps: bool,

    /// Дополнительно удалить замкнутые просветы фона (между ногами, между рукой и
    /// телом), которые заливка от краёв не достаёт. Включайте осторожно: большая
    /// замкнутая область белого цвета на самом объекте (одежда) тоже будет удалена.
    /// Требует --remove-bg или --cutbg
    #[arg(long)]
    pub remove_holes: bool,

    /// Минимальная площадь просвета для --remove-holes, % площади изображения
    #[arg(long, default_value_t = 0.04)]
    pub hole_area: f32,

    /// Удалять изолированные группы ячеек меньше N на готовой сетке (0 — выключено).
    /// Самая большая группа сохраняется всегда
    #[arg(long, default_value_t = 2)]
    pub despeckle: u32,

    /// Префикс выходных файлов (по умолчанию — имя входного файла;
    /// сохраняются в папке входного файла, если в префиксе нет пути)
    #[arg(short, long, value_name = "NAME")]
    pub prefix: Option<String>,
}

impl Args {
    /// Вырезание фона включено (`--remove-bg` или `--cutbg`).
    pub fn removes_background(&self) -> bool {
        self.remove_bg || self.cutbg
    }

    /// Параметры удаления фона из флагов CLI.
    pub fn background_options(&self) -> BackgroundOptions {
        BackgroundOptions {
            color: self.bg_color,
            tolerance: self.bg_tolerance,
            grow: self.bg_grow,
            defringe: self.bg_defringe,
            thin_gaps: self.thin_gaps,
            remove_holes: self.remove_holes,
            hole_area_pct: self.hole_area,
        }
    }

    /// Параметры полного преобразования для ядра.
    pub fn options(&self) -> Options {
        Options {
            width: self.width,
            filter: self.filter.into(),
            colors: self.colors,
            cell: self.cell,
            grid_opacity: self.grid_opacity,
            grid_color: self.grid_color,
            alpha_threshold: self.alpha_threshold,
            fill: self.background,
            despeckle: self.despeckle,
            background_removal: self.removes_background().then(|| self.background_options()),
        }
    }
}
