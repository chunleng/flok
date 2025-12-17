use std::{fs::File, path::PathBuf, time::Duration};

use anyhow::anyhow;
use clap::{Parser, Subcommand};
use error::{FlokConfigError, FlokError};
use serde::Deserialize;

mod error;
mod ui;
mod watcher;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short, long, default_value=None)]
    config_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Version,
}

fn main() {
    match process_cmd() {
        Ok(_) => {}
        Err(e) => {
            println!("{}", e.to_string());
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Config {
    flocks: Vec<Flock>,
}

#[derive(Debug, Clone, Deserialize)]
struct Flock {
    display_name: String,
    processes: Vec<FlockProcess>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum WatchConfig {
    Enabled(bool),
    WithDebounce { debounce_seconds: Option<f64> },
}

impl Default for WatchConfig {
    fn default() -> Self {
        WatchConfig::Enabled(false)
    }
}

impl WatchConfig {
    fn is_enabled(&self) -> bool {
        match self {
            WatchConfig::Enabled(enabled) => *enabled,
            WatchConfig::WithDebounce { .. } => true,
        }
    }

    fn debounce_duration(&self) -> Duration {
        match self {
            WatchConfig::Enabled(true) => Duration::from_secs(2),
            WatchConfig::Enabled(false) => Duration::from_secs(0),
            WatchConfig::WithDebounce { debounce_seconds } => {
                Duration::from_secs_f64(debounce_seconds.unwrap_or(1.0))
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct FlockProcess {
    display_name: String,
    command: String,
    #[serde(default)]
    watch: WatchConfig,
}

fn process_config(cli: Cli) -> Result<Config, FlokConfigError> {
    let config_file = cli.config_file.unwrap_or("./flok.yaml".into());

    Ok(serde_yaml::from_reader(
        File::open(config_file.clone()).map_err(move |_| {
            // TODO more fine grain error handling
            anyhow!(format!(
                "Unable to open \"{}\", please check if it exists and is readable",
                config_file.to_string_lossy().to_string()
            ))
        })?,
    )?)
}

fn process_cmd() -> Result<(), FlokError> {
    let args = Cli::try_parse();
    match args {
        Ok(args) if args.command.is_some() => show_version(),
        Ok(args) => {
            ui::run(process_config(args)?)?;
        }
        Err(msg) => {
            let _ = msg.print();
        }
    }

    Ok(())
}

fn show_version() {
    println!("Flok version v{}", env!("CARGO_PKG_VERSION"));
}
