//! User preferences and the runtime forest path.
//!
//! Preferences live in `~/.config/forestui/settings.json` (global). The forest
//! path is a CLI argument, not a setting, so multiple forests stay independent.

use crate::models::Settings;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

static FOREST_PATH: OnceLock<RwLock<PathBuf>> = OnceLock::new();

fn forest_cell() -> &'static RwLock<PathBuf> {
    FOREST_PATH.get_or_init(|| RwLock::new(default_forest_path()))
}

pub fn default_forest_path() -> PathBuf {
    crate::util::home_dir().join("forest")
}

/// Set the runtime forest path. `None` restores the default `~/forest`.
pub fn set_forest_path(path: Option<&str>) {
    let resolved = match path {
        None => default_forest_path(),
        Some(p) => crate::util::expand_and_resolve(p),
    };
    if let Ok(mut guard) = forest_cell().write() {
        *guard = resolved;
    }
}

pub fn get_forest_path() -> PathBuf {
    forest_cell()
        .read()
        .map(|p| p.clone())
        .unwrap_or_else(|_| default_forest_path())
}

pub fn settings_path() -> PathBuf {
    crate::util::home_dir()
        .join(".config")
        .join("forestui")
        .join("settings.json")
}

/// Load settings, falling back to defaults for a missing or unreadable file.
pub fn load_settings() -> Settings {
    load_settings_from(&settings_path())
}

pub fn load_settings_from(path: &Path) -> Settings {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Settings::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    save_settings_to(&settings_path(), settings)
}

pub fn save_settings_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::util::write_atomically(path, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let s = load_settings_from(Path::new("/nonexistent/settings.json"));
        assert_eq!(s.default_editor, "vim");
        assert_eq!(s.branch_prefix, "feat/");
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, r#"{"default_editor": "nvim"}"#).unwrap();
        let s = load_settings_from(&p);
        assert_eq!(s.default_editor, "nvim");
        assert_eq!(s.theme, "forest-dark");
        assert!(s.custom_buttons.is_empty());
    }

    #[test]
    fn roundtrip_keeps_python_field_names() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        let s = Settings {
            default_editor: "hx".into(),
            ..Settings::default()
        };
        save_settings_to(&p, &s).unwrap();
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(raw.contains("\"default_editor\""));
        assert!(raw.contains("\"branch_prefix\""));
        assert!(raw.contains("\"custom_buttons\""));
        assert_eq!(load_settings_from(&p).default_editor, "hx");
    }
}
