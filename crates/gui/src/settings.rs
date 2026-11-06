//! Persistent GUI settings.
//!
//! The file is a versioned JSON document written atomically (temporary file
//! plus rename) after the user stops changing controls. A corrupt document is
//! moved aside to `.bak` and the defaults are used instead.
//!
//! Layout follows `docs/SPEC.md` §5.11:
//!
//! - Windows: `%APPDATA%\pixel-image\gui-settings.json`
//! - Linux: `$XDG_CONFIG_HOME/pixel-image/gui-settings.json` (or
//!   `~/.config/pixel-image/gui-settings.json`)
//! - macOS: `~/Library/Application Support/pixel-image/gui-settings.json`

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use pixel_image_core::{Filter, color::parse_hex};
use serde::{Deserialize, Serialize};

use crate::model::{Mode, Model, OutputParams, PixelParams};

/// Current schema version written by this build.
pub const VERSION: u32 = 1;
/// How many recent files are remembered.
pub const MAX_RECENT: usize = 10;
/// File name inside the per-user configuration directory.
pub const FILE_NAME: &str = "gui-settings.json";

/// Editor settings as stored on disk.
///
/// Every field has a default, so an older or partially hand-edited document
/// still loads. Values are clamped to the GUI ranges when applied to the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub pixel: PixelSettings,
    #[serde(default)]
    pub background: BackgroundSettings,
    #[serde(default)]
    pub output: OutputSettings,
    #[serde(default)]
    pub window: WindowSettings,
    #[serde(default)]
    pub recent: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: VERSION,
            mode: default_mode(),
            pixel: PixelSettings::default(),
            background: BackgroundSettings::default(),
            output: OutputSettings::default(),
            window: WindowSettings::default(),
            recent: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PixelSettings {
    pub width: u32,
    pub filter: String,
    pub colors: u16,
    pub cell: u32,
    pub grid_opacity: u8,
    pub grid_color: String,
    pub alpha_threshold: u8,
    /// `Some(RRGGBB)` turns the fill on; `None` keeps it off.
    pub fill: Option<String>,
    pub despeckle: u32,
}

impl Default for PixelSettings {
    fn default() -> Self {
        let params = PixelParams::default();
        Self {
            width: params.width,
            filter: filter_key(params.filter).to_string(),
            colors: params.colors,
            cell: params.cell,
            grid_opacity: params.grid_opacity,
            grid_color: hex_key(params.grid_color),
            alpha_threshold: params.alpha_threshold,
            fill: params.fill_enabled.then(|| hex_key(params.fill_color)),
            despeckle: params.despeckle,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundSettings {
    /// The "УДАЛИТЬ ФОН" checkbox of the pixel-art mode.
    pub enabled: bool,
    pub auto_color: bool,
    pub color: String,
    pub tolerance: u8,
    pub grow: u32,
    pub defringe_auto: bool,
    pub defringe: u32,
    pub thin_gaps: bool,
    pub remove_holes: bool,
    pub hole_area_pct: f32,
}

impl Default for BackgroundSettings {
    fn default() -> Self {
        let params = PixelParams::default();
        Self {
            enabled: params.remove_bg,
            auto_color: params.bg_auto_color,
            color: hex_key(params.bg_color),
            tolerance: params.bg_tolerance,
            grow: params.bg_grow,
            defringe_auto: params.bg_auto_defringe,
            defringe: params.bg_defringe,
            thin_gaps: params.thin_gaps,
            remove_holes: params.remove_holes,
            hole_area_pct: params.hole_area_pct,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    /// `None` follows the source folder.
    pub dir: Option<String>,
    /// `None` follows the source file name.
    pub prefix: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    /// Last floating (not maximized) client size.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The window filled the monitor when the document was written. The next
    /// start creates the window maximized instead of restoring the maximized
    /// client size as a floating window (which would not fit the screen once
    /// the title bar and borders are added).
    pub maximized: bool,
}

/// Result of reading the settings file at startup.
pub struct Loaded {
    pub settings: Settings,
    /// Human-readable problem to show as a toast; `None` for a clean load.
    pub problem: Option<String>,
}

impl Settings {
    /// Capture the current editor state.
    pub fn from_model(model: &Model, window: Option<(u32, u32)>, maximized: bool) -> Self {
        let params = &model.params;
        Self {
            version: VERSION,
            mode: mode_key(model.mode).to_string(),
            pixel: PixelSettings {
                width: params.width,
                filter: filter_key(params.filter).to_string(),
                colors: params.colors,
                cell: params.cell,
                grid_opacity: params.grid_opacity,
                grid_color: hex_key(params.grid_color),
                alpha_threshold: params.alpha_threshold,
                fill: params.fill_enabled.then(|| hex_key(params.fill_color)),
                despeckle: params.despeckle,
            },
            background: BackgroundSettings {
                enabled: params.remove_bg,
                auto_color: params.bg_auto_color,
                color: hex_key(params.bg_color),
                tolerance: params.bg_tolerance,
                grow: params.bg_grow,
                defringe_auto: params.bg_auto_defringe,
                defringe: params.bg_defringe,
                thin_gaps: params.thin_gaps,
                remove_holes: params.remove_holes,
                hole_area_pct: params.hole_area_pct,
            },
            output: OutputSettings {
                dir: model
                    .output
                    .dir
                    .as_ref()
                    .map(|dir| dir.to_string_lossy().into_owned()),
                prefix: model.output.prefix.clone(),
            },
            window: WindowSettings {
                width: window.map(|(width, _)| width),
                height: window.map(|(_, height)| height),
                maximized,
            },
            recent: model
                .recent
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        }
    }

    /// Apply the document to the model, clamping every value to a safe range.
    pub fn apply(&self, model: &mut Model) {
        let params = &mut model.params;
        params.width = self.pixel.width.clamp(1, 512);
        params.filter = parse_filter(&self.pixel.filter);
        params.colors = self.pixel.colors.clamp(2, 256);
        params.cell = self.pixel.cell.clamp(2, 64);
        params.grid_opacity = self.pixel.grid_opacity.min(100);
        params.grid_color = parse_hex(&self.pixel.grid_color).unwrap_or([0, 0, 0]);
        params.alpha_threshold = self.pixel.alpha_threshold.clamp(1, 255);
        match &self.pixel.fill {
            Some(fill) => {
                params.fill_enabled = true;
                params.fill_color = parse_hex(fill).unwrap_or([255, 255, 255]);
            }
            None => params.fill_enabled = false,
        }
        params.despeckle = self.pixel.despeckle.min(32);

        params.remove_bg = self.background.enabled;
        params.bg_auto_color = self.background.auto_color;
        params.bg_color = parse_hex(&self.background.color).unwrap_or([255, 255, 255]);
        params.bg_tolerance = self.background.tolerance;
        params.bg_grow = self.background.grow.min(16);
        params.bg_auto_defringe = self.background.defringe_auto;
        params.bg_defringe = self.background.defringe.min(64);
        params.thin_gaps = self.background.thin_gaps;
        params.remove_holes = self.background.remove_holes;
        params.hole_area_pct = if self.background.hole_area_pct.is_finite() {
            (self.background.hole_area_pct * 1000.0).round().max(1.0) / 1000.0
        } else {
            0.04
        }
        .clamp(0.001, 10.0);

        model.mode = match self.mode.as_str() {
            "cutout" => Mode::Cutout,
            _ => Mode::PixelArt,
        };

        let dir = self
            .output
            .dir
            .as_deref()
            .map(PathBuf::from)
            .filter(|dir| !dir.as_os_str().is_empty());
        model.output = OutputParams {
            dir,
            prefix: self
                .output
                .prefix
                .as_deref()
                .filter(|prefix| valid_prefix(prefix))
                .map(str::to_string),
        };

        model.recent = self
            .recent
            .iter()
            .filter(|entry| !entry.is_empty())
            .map(PathBuf::from)
            .fold(Vec::new(), |mut recent, path| {
                if !recent.contains(&path) {
                    recent.push(path);
                }
                recent
            });
        model.recent.truncate(MAX_RECENT);
    }

    /// Saved window size, if it was stored and is above the minimum size.
    pub fn window_size(&self) -> Option<(u32, u32)> {
        let width = self.window.width?;
        let height = self.window.height?;
        (width >= 640 && height >= 400).then_some((width, height))
    }

    /// Bring older documents up to [`VERSION`]; newer ones are rejected.
    fn migrate(mut self) -> Result<Self, String> {
        if self.version == 0 {
            // Version 0 is the document written before the version field
            // existed; it has the same shape as version 1.
            self.version = 1;
        }
        if self.version > VERSION {
            return Err(format!(
                "НАСТРОЙКИ НОВЕЕ ЭТОЙ ВЕРСИИ ПРОГРАММЫ (СХЕМА {}), СБРОШЕНЫ",
                self.version
            ));
        }
        Ok(self)
    }
}

/// Path of the settings document for the current user.
pub fn settings_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    };
    base.map(|base| base.join("pixel-image").join(FILE_NAME))
}

/// Read the document; a missing file is not an error.
pub fn load(path: Option<&Path>) -> Loaded {
    let Some(path) = path else {
        return Loaded {
            settings: Settings::default(),
            problem: None,
        };
    };
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Settings>(&text) {
            Ok(settings) => match settings.migrate() {
                Ok(settings) => Loaded {
                    settings,
                    problem: None,
                },
                Err(problem) => {
                    backup(path);
                    Loaded {
                        settings: Settings::default(),
                        problem: Some(problem),
                    }
                }
            },
            Err(error) => {
                let backup = backup(path);
                let mut problem = format!("НАСТРОЙКИ ПОВРЕЖДЕНЫ И СБРОШЕНЫ\n{error}");
                if let Some(backup) = backup {
                    problem.push_str(&format!("\nКОПИЯ: {}", backup.display()));
                }
                Loaded {
                    settings: Settings::default(),
                    problem: Some(problem),
                }
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Loaded {
            settings: Settings::default(),
            problem: None,
        },
        Err(error) => Loaded {
            settings: Settings::default(),
            problem: Some(format!(
                "НЕ УДАЛОСЬ ПРОЧИТАТЬ НАСТРОЙКИ\n{}\n{error}",
                path.display()
            )),
        },
    }
}

/// Write the document atomically: temporary file in the same directory, then
/// rename over the target.
pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "НЕ УДАЛОСЬ ОПРЕДЕЛИТЬ ПАПКУ НАСТРОЕК ДЛЯ {}",
            path.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "НЕ УДАЛОСЬ СОЗДАТЬ ПАПКУ НАСТРОЕК\n{}\n{error}",
            parent.display()
        )
    })?;

    let text = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("НЕ УДАЛОСЬ СЕРИАЛИЗОВАТЬ НАСТРОЙКИ\n{error}"))?;
    // `gui-settings.json` -> `gui-settings.json.tmp` (same directory, so the
    // rename below stays on one filesystem and is atomic).
    let temporary = path.with_extension("json.tmp");
    let write = || -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()
    };
    if let Err(error) = write() {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "НЕ УДАЛОСЬ ЗАПИСАТЬ НАСТРОЙКИ\n{}\n{error}",
            temporary.display()
        ));
    }
    if let Err(error) = replace(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "НЕ УДАЛОСЬ ЗАМЕНИТЬ НАСТРОЙКИ\n{}\n{error}",
            path.display()
        ));
    }
    Ok(())
}

/// Rename `temporary` over `target`. Windows refuses to rename onto an
/// existing file, so the old document is removed first as a fallback.
fn replace(temporary: &Path, target: &Path) -> std::io::Result<()> {
    match fs::rename(temporary, target) {
        Ok(()) => Ok(()),
        Err(first) => {
            if target.exists() {
                fs::remove_file(target)?;
                fs::rename(temporary, target)
            } else {
                Err(first)
            }
        }
    }
}

/// Move a broken document out of the way; returns the backup path.
fn backup(path: &Path) -> Option<PathBuf> {
    let backup = path.with_extension("json.bak");
    let _ = fs::remove_file(&backup);
    fs::rename(path, &backup).ok().map(|()| backup)
}

fn default_version() -> u32 {
    VERSION
}

fn default_mode() -> String {
    "pixel_art".to_string()
}

fn mode_key(mode: Mode) -> &'static str {
    match mode {
        Mode::PixelArt => "pixel_art",
        Mode::Cutout => "cutout",
    }
}

fn filter_key(filter: Filter) -> &'static str {
    match filter {
        Filter::Lanczos => "lanczos",
        Filter::Catmull => "catmull",
        Filter::Triangle => "triangle",
        Filter::Gaussian => "gaussian",
        Filter::Nearest => "nearest",
        Filter::Box => "box",
        Filter::Mode => "mode",
    }
}

fn parse_filter(text: &str) -> Filter {
    match text.trim().to_ascii_lowercase().as_str() {
        "catmull" => Filter::Catmull,
        "triangle" => Filter::Triangle,
        "gaussian" => Filter::Gaussian,
        "nearest" => Filter::Nearest,
        "box" => Filter::Box,
        "mode" => Filter::Mode,
        _ => Filter::Lanczos,
    }
}

fn hex_key(color: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", color[0], color[1], color[2])
}

/// Prefixes must stay a single file name, because the core appends
/// `_grid.png` / `_sprite.png` / `_cut.png` to them.
fn valid_prefix(prefix: &str) -> bool {
    let prefix = prefix.trim();
    !prefix.is_empty()
        && !prefix
            .chars()
            .any(|ch| ch == '/' || ch == '\\' || ch.is_control())
        && !(cfg!(windows)
            && prefix
                .chars()
                .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_document_without_maximized_still_loads() {
        let document = r#"{"version":1,"window":{"width":1280,"height":800}}"#;
        let settings: Settings = serde_json::from_str(document).expect("old document");

        assert!(!settings.window.maximized);
        assert_eq!(settings.window_size(), Some((1280, 800)));
    }

    #[test]
    fn maximized_state_round_trips() {
        let settings = Settings {
            window: WindowSettings {
                width: Some(1280),
                height: Some(800),
                maximized: true,
            },
            ..Settings::default()
        };

        let text = serde_json::to_string(&settings).expect("serialize");
        let restored: Settings = serde_json::from_str(&text).expect("deserialize");

        assert!(restored.window.maximized);
        assert_eq!(restored.window_size(), Some((1280, 800)));
    }
}
