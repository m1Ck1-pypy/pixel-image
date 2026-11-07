//! pixel-image-core — переиспользуемая логика превращения изображения в
//! пиксель-арт и вырезания однотонного фона.
//!
//! Крейт не знает про консоль и GUI: он не печатает сообщения и не разбирает
//! аргументы. Пользовательский интерфейс (CLI, GUI) собирает [`Options`] или
//! [`BackgroundOptions`], вызывает [`convert_file`] / [`cutout_file`] и сам
//! показывает статистику из [`Stats`] / [`CleanStats`].
//!
//! Два уровня API:
//!
//! - файловый — [`convert_file`] и [`cutout_file`]: читают файл, обрабатывают,
//!   сохраняют PNG и возвращают отчёт;
//! - в памяти — [`convert_image`] и [`cutout_image`]: принимают `RgbaImage` и
//!   возвращают изображения без записи на диск (пригодится для GUI-превью).

pub mod clean;
pub mod color;
pub mod downscale;
pub mod error;
pub mod grid;
pub mod palette;
pub mod pipeline;

pub use color::Color;
pub use downscale::Filter;
pub use error::{Error, Result};
pub use pipeline::{
    BackgroundOptions, BackgroundStats, CleanStats, Conversion, Cutout, Options, SavedConversion,
    SavedCutout, StageTimes, Stats, clean_source, convert_file, convert_image, convert_image_timed,
    cutout_file, cutout_image, output_path, save_png,
};
