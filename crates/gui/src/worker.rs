//! Background file loading, generation, cutout and saving.
//!
//! One worker thread serialises every heavy operation. Every request carries a
//! job id and the file generation it belongs to, so the UI can drop results
//! that are no longer relevant.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use image::RgbaImage;
use pixel_image_core::{BackgroundOptions, CleanStats, Error, Options, Stats};
use winit::event_loop::EventLoopProxy;

/// Longest side of the cutout copy used for the screen. Export always uses the
/// full-resolution core path, so this only limits preview memory and paint cost.
const DISPLAY_LIMIT: u32 = 2048;

/// Wake-up event for the winit event loop.
pub enum UserEvent {
    /// A worker result is ready (or the worker finished a job).
    Worker,
    /// The asynchronous native dialog made progress; the event loop polls it.
    Dialog,
}

#[derive(Clone, Debug)]
pub struct FileMeta {
    pub format: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

pub enum WorkerResult {
    Loaded {
        generation: u64,
        path: PathBuf,
        image: Arc<RgbaImage>,
        meta: FileMeta,
    },
    LoadFailed {
        generation: u64,
        message: String,
    },
    Generated {
        job: u64,
        file: u64,
        sprite: Arc<RgbaImage>,
        stats: Stats,
        elapsed: Duration,
    },
    GenerateFailed {
        job: u64,
        file: u64,
        message: String,
    },
    Cutout {
        job: u64,
        file: u64,
        /// Downscaled copy for the screen, not the export result.
        display: Arc<RgbaImage>,
        width: u32,
        height: u32,
        stats: CleanStats,
        fully_transparent: bool,
        elapsed: Duration,
    },
    CutoutFailed {
        job: u64,
        file: u64,
        message: String,
    },
    Saved {
        job: u64,
        file: u64,
        grid_path: PathBuf,
        sprite_path: PathBuf,
        stats: Stats,
        elapsed: Duration,
    },
    CutoutSaved {
        job: u64,
        file: u64,
        path: PathBuf,
        width: u32,
        height: u32,
        stats: CleanStats,
        fully_transparent: bool,
        elapsed: Duration,
    },
    SaveFailed {
        job: u64,
        file: u64,
        message: String,
    },
}

enum Job {
    Load {
        path: PathBuf,
        generation: u64,
    },
    Generate {
        job: u64,
        file: u64,
        source: Arc<RgbaImage>,
        options: Options,
    },
    Cutout {
        job: u64,
        file: u64,
        source: Arc<RgbaImage>,
        background: BackgroundOptions,
    },
    Save {
        job: u64,
        file: u64,
        path: PathBuf,
        /// Absolute base path for export; `None` writes next to the source.
        prefix: Option<String>,
        options: Options,
    },
    SaveCutout {
        job: u64,
        file: u64,
        path: PathBuf,
        /// Absolute base path for export; `None` writes next to the source.
        prefix: Option<String>,
        background: BackgroundOptions,
    },
    Shutdown,
}

pub struct Worker {
    jobs: Sender<Job>,
    results: Receiver<WorkerResult>,
    _thread: JoinHandle<()>,
}

impl Worker {
    pub fn spawn(proxy: EventLoopProxy<UserEvent>) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<WorkerResult>();

        let thread = thread::spawn(move || {
            while let Ok(job) = job_rx.recv() {
                let result = match job {
                    Job::Shutdown => break,
                    Job::Load { path, generation } => {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            load(&path, generation)
                        }))
                        .unwrap_or_else(|_| WorkerResult::LoadFailed {
                            generation,
                            message: "ВНУТРЕННЯЯ ОШИБКА ЗАГРУЗКИ".to_string(),
                        })
                    }
                    Job::Generate {
                        job,
                        file,
                        source,
                        options,
                    } => generate(job, file, &source, &options),
                    Job::Cutout {
                        job,
                        file,
                        source,
                        background,
                    } => cutout(job, file, &source, &background),
                    Job::Save {
                        job,
                        file,
                        path,
                        prefix,
                        options,
                    } => save(job, file, &path, prefix.as_deref(), &options),
                    Job::SaveCutout {
                        job,
                        file,
                        path,
                        prefix,
                        background,
                    } => save_cutout(job, file, &path, prefix.as_deref(), &background),
                };
                if result_tx.send(result).is_ok() {
                    let _ = proxy.send_event(UserEvent::Worker);
                }
            }
        });

        Self {
            jobs: job_tx,
            results: result_rx,
            _thread: thread,
        }
    }

    pub fn load(&self, path: PathBuf, generation: u64) -> bool {
        self.jobs.send(Job::Load { path, generation }).is_ok()
    }

    pub fn generate(&self, job: u64, file: u64, source: Arc<RgbaImage>, options: Options) -> bool {
        self.jobs
            .send(Job::Generate {
                job,
                file,
                source,
                options,
            })
            .is_ok()
    }

    pub fn cutout(
        &self,
        job: u64,
        file: u64,
        source: Arc<RgbaImage>,
        background: BackgroundOptions,
    ) -> bool {
        self.jobs
            .send(Job::Cutout {
                job,
                file,
                source,
                background,
            })
            .is_ok()
    }

    pub fn save(
        &self,
        job: u64,
        file: u64,
        path: PathBuf,
        prefix: Option<String>,
        options: Options,
    ) -> bool {
        self.jobs
            .send(Job::Save {
                job,
                file,
                path,
                prefix,
                options,
            })
            .is_ok()
    }

    pub fn save_cutout(
        &self,
        job: u64,
        file: u64,
        path: PathBuf,
        prefix: Option<String>,
        background: BackgroundOptions,
    ) -> bool {
        self.jobs
            .send(Job::SaveCutout {
                job,
                file,
                path,
                prefix,
                background,
            })
            .is_ok()
    }

    pub fn poll(&self) -> Option<WorkerResult> {
        self.results.try_recv().ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.jobs.send(Job::Shutdown);
    }
}

fn load(path: &Path, generation: u64) -> WorkerResult {
    let bytes = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    match image::open(path) {
        Ok(image) => {
            let (width, height) = (image.width(), image.height());
            let image = image.into_rgba8();
            WorkerResult::Loaded {
                generation,
                path: path.to_path_buf(),
                image: Arc::new(image),
                meta: FileMeta {
                    format: format_name(path),
                    bytes,
                    width,
                    height,
                },
            }
        }
        Err(error) => WorkerResult::LoadFailed {
            generation,
            message: format!("НЕ УДАЛОСЬ ОТКРЫТЬ ФАЙЛ\n{}\n{}", path.display(), error),
        },
    }
}

fn generate(job: u64, file: u64, source: &Arc<RgbaImage>, options: &Options) -> WorkerResult {
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pixel_image_core::convert_image(source.as_ref(), options)
    }))
    .unwrap_or_else(|_| Err(Error::msg("ВНУТРЕННЯЯ ОШИБКА ГЕНЕРАЦИИ")));
    let elapsed = started.elapsed();
    match outcome {
        Ok(conversion) => WorkerResult::Generated {
            job,
            file,
            sprite: Arc::new(conversion.sprite),
            stats: conversion.stats,
            elapsed,
        },
        Err(error) => WorkerResult::GenerateFailed {
            job,
            file,
            message: format!("НЕ УДАЛОСЬ СГЕНЕРИРОВАТЬ ИЗОБРАЖЕНИЕ\n{error}"),
        },
    }
}

fn cutout(
    job: u64,
    file: u64,
    source: &Arc<RgbaImage>,
    background: &BackgroundOptions,
) -> WorkerResult {
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pixel_image_core::cutout_image((**source).clone(), background)
    }))
    .unwrap_or_else(|_| Err(Error::msg("ВНУТРЕННЯЯ ОШИБКА ВЫРЕЗКИ")));
    let elapsed = started.elapsed();
    match outcome {
        Ok(cutout) => {
            let (width, height) = cutout.image.dimensions();
            WorkerResult::Cutout {
                job,
                file,
                display: Arc::new(shrink_for_display(cutout.image)),
                width,
                height,
                stats: cutout.stats,
                fully_transparent: cutout.fully_transparent,
                elapsed,
            }
        }
        Err(error) => WorkerResult::CutoutFailed {
            job,
            file,
            message: format!("НЕ УДАЛОСЬ ВЫРЕЗАТЬ ФОН\n{error}"),
        },
    }
}

fn save(job: u64, file: u64, path: &Path, prefix: Option<&str>, options: &Options) -> WorkerResult {
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pixel_image_core::convert_file(path, options, prefix)
    }))
    .unwrap_or_else(|_| Err(Error::msg("ВНУТРЕННЯЯ ОШИБКА СОХРАНЕНИЯ")));
    let elapsed = started.elapsed();
    match outcome {
        Ok(saved) => WorkerResult::Saved {
            job,
            file,
            grid_path: saved.grid_path,
            sprite_path: saved.sprite_path,
            stats: saved.stats,
            elapsed,
        },
        Err(error) => WorkerResult::SaveFailed {
            job,
            file,
            message: format!("НЕ УДАЛОСЬ СОХРАНИТЬ ФАЙЛЫ\n{}\n{error}", path.display()),
        },
    }
}

fn save_cutout(
    job: u64,
    file: u64,
    path: &Path,
    prefix: Option<&str>,
    background: &BackgroundOptions,
) -> WorkerResult {
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pixel_image_core::cutout_file(path, background, prefix)
    }))
    .unwrap_or_else(|_| Err(Error::msg("ВНУТРЕННЯЯ ОШИБКА СОХРАНЕНИЯ")));
    let elapsed = started.elapsed();
    match outcome {
        Ok(saved) => WorkerResult::CutoutSaved {
            job,
            file,
            path: saved.path,
            width: saved.width,
            height: saved.height,
            stats: saved.stats,
            fully_transparent: saved.fully_transparent,
            elapsed,
        },
        Err(error) => WorkerResult::SaveFailed {
            job,
            file,
            message: format!("НЕ УДАЛОСЬ СОХРАНИТЬ ВЫРЕЗКУ\n{}\n{error}", path.display()),
        },
    }
}

/// Downscale a cutout copy for the screen.
///
/// The long side is capped at 4096 px, which keeps the preview buffer bounded
/// on huge photos. The export result is always produced by the core from the
/// file at full resolution.
pub fn shrink_for_display(image: RgbaImage) -> RgbaImage {
    let (width, height) = image.dimensions();
    let longest = width.max(height);
    if longest <= DISPLAY_LIMIT {
        return image;
    }
    let ratio = f64::from(DISPLAY_LIMIT) / f64::from(longest);
    let new_width = ((f64::from(width) * ratio).round() as u32).max(1);
    let new_height = ((f64::from(height) * ratio).round() as u32).max(1);
    image::imageops::resize(
        &image,
        new_width,
        new_height,
        image::imageops::FilterType::Triangle,
    )
}

fn format_name(path: &Path) -> String {
    match image::ImageFormat::from_path(path) {
        Ok(image::ImageFormat::Png) => "PNG".to_string(),
        Ok(image::ImageFormat::Jpeg) => "JPEG".to_string(),
        Ok(image::ImageFormat::WebP) => "WEBP".to_string(),
        Ok(image::ImageFormat::Bmp) => "BMP".to_string(),
        Ok(image::ImageFormat::Gif) => "GIF".to_string(),
        Ok(other) => format!("{other:?}").to_uppercase(),
        Err(_) => "ИЗОБРАЖЕНИЕ".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]);
        }
        img
    }

    #[test]
    fn shrink_for_display_limits() {
        // Меньше лимита — не трогаем (ни размеров, ни пикселей).
        let small = solid(300, 200);
        let same = shrink_for_display(small.clone());
        assert_eq!(same.dimensions(), (300, 200));
        assert_eq!(same.as_raw(), small.as_raw());

        // Больше лимита — длинная сторона ровно DISPLAY_LIMIT, пропорции те же.
        let shrunk = shrink_for_display(solid(2400, 1800));
        assert_eq!(shrunk.dimensions(), (DISPLAY_LIMIT, DISPLAY_LIMIT * 3 / 4));

        let portrait = shrink_for_display(solid(1200, 3200));
        assert_eq!(portrait.dimensions(), (DISPLAY_LIMIT * 3 / 8, DISPLAY_LIMIT));
    }

    #[test]
    fn shrink_for_display_is_idempotent() {
        let once = shrink_for_display(solid(2400, 1800));
        let twice = shrink_for_display(once.clone());
        assert_eq!(twice.dimensions(), once.dimensions());
        assert_eq!(twice.as_raw(), once.as_raw());
    }
}
