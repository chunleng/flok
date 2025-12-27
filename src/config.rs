use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub flocks: Vec<FlockConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlockConfig {
    pub display_name: String,
    pub processes: Vec<FlockProcessConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlockProcessConfig {
    pub display_name: String,
    pub command: String,
    #[serde(default)]
    pub watch: WatchConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum WatchConfig {
    Enabled(bool),
    WithDebounce { debounce_seconds: Option<f64> },
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
            WatchConfig::WithDebounce { .. } => true,
        }
    }

    pub fn debounce_duration(&self) -> Duration {
        match self {
            WatchConfig::Enabled(true) => Duration::from_secs(2),
            WatchConfig::Enabled(false) => Duration::from_secs(0),
            WatchConfig::WithDebounce { debounce_seconds } => {
                Duration::from_secs_f64(debounce_seconds.unwrap_or(1.0))
            }
        }
    }
}
