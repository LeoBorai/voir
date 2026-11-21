//! User settings, read once at startup from `~/.dato/settings.toml`. A missing
//! file is created with the defaults; an unreadable or malformed one falls
//! back to the defaults with a warning, so a bad config never blocks queries.

use std::path::PathBuf;
use std::sync::OnceLock;

const DEFAULT_FILE: &str = "# Colorize table output on a terminal.\ncolors = true\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub colors: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { colors: true }
    }
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();

/// Loads (creating if needed) the settings file and makes it the process-wide
/// settings. Call once at startup; later calls are ignored.
pub fn init() {
    let _ = SETTINGS.set(load(settings_path()));
}

pub fn get() -> Settings {
    *SETTINGS.get_or_init(Settings::default)
}

fn settings_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".dato").join("settings.toml"))
}

fn load(path: Option<PathBuf>) -> Settings {
    let Some(path) = path else {
        return Settings::default();
    };
    if !path.exists() {
        let created = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, DEFAULT_FILE));
        if let Err(err) = created {
            eprintln!("warning: cannot create {}: {err}", path.display());
        }
        return Settings::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).unwrap_or_else(|err| {
            eprintln!("warning: {}: {err}; using defaults", path.display());
            Settings::default()
        }),
        Err(err) => {
            eprintln!("warning: cannot read {}: {err}", path.display());
            Settings::default()
        }
    }
}

fn parse(text: &str) -> Result<Settings, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut settings = Settings::default();
    if let Some(value) = table.get("colors") {
        settings.colors = value
            .as_bool()
            .ok_or_else(|| "`colors` must be true or false".to_string())?;
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_colors_on() {
        assert!(parse("").unwrap().colors);
        assert!(parse(DEFAULT_FILE).unwrap().colors);
    }

    #[test]
    fn colors_can_be_disabled() {
        assert!(!parse("colors = false").unwrap().colors);
    }

    #[test]
    fn rejects_non_boolean_colors() {
        assert!(parse("colors = \"yes\"").is_err());
    }

    #[test]
    fn missing_file_is_created_with_defaults() {
        let dir = std::env::temp_dir().join(format!("voir-settings-{}", std::process::id()));
        let path = dir.join(".dato").join("settings.toml");
        assert_eq!(load(Some(path.clone())), Settings::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_FILE);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
