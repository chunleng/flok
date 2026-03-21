use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde_valid::Validate;

#[derive(Debug, Clone, Deserialize, Validate)]
pub struct AppConfig {
    #[validate(min_items = 1)]
    pub flocks: Vec<FlockConfig>,
    #[serde(default)]
    pub processes: Vec<ProcessConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlockConfig {
    pub display_name: String,
    pub processes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessConfig {
    pub id: String,
    pub display_name: String,
    pub command: String,
    #[serde(default)]
    pub watch: WatchConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum WatchConfig {
    Enabled(bool),
    WithOptions {
        debounce_seconds: Option<f64>,
        path: Option<String>,
    },
}

impl Default for WatchConfig {
    fn default() -> Self {
        WatchConfig::Enabled(false)
    }
}

impl WatchConfig {
    pub fn is_enabled(&self) -> bool {
        match self {
            WatchConfig::Enabled(enabled) => *enabled,
            WatchConfig::WithOptions { .. } => true,
        }
    }

    pub fn debounce_duration(&self) -> Duration {
        match self {
            WatchConfig::Enabled(true) => Duration::from_secs(2),
            WatchConfig::Enabled(false) => Duration::from_secs(0),
            WatchConfig::WithOptions {
                debounce_seconds, ..
            } => Duration::from_secs_f64(debounce_seconds.unwrap_or(1.0)),
        }
    }

    pub fn resolve_watch_path(&self) -> Option<PathBuf> {
        let cwd = PathBuf::from(".");
        match self {
            WatchConfig::Enabled(true) => Some(cwd),
            WatchConfig::Enabled(false) => None,
            WatchConfig::WithOptions { path, .. } => match path {
                Some(p) => {
                    let path = PathBuf::from(p);
                    if path.is_absolute() {
                        Some(path)
                    } else {
                        Some(cwd.join(path))
                    }
                }
                None => Some(cwd),
            },
        }
    }
}
