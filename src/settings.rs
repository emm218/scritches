use clap::Parser;
use config::Config;
use serde_derive::Deserialize;

use std::{path::PathBuf, result::Result};

#[derive(Debug, Parser)]
#[command(version)]
pub struct Args {
    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// TCP socket address for MPD
    #[arg(short, long)]
    addr: Option<String>,

    /// Unix socket for MPD
    #[arg(short, long)]
    socket: Option<String>,

    /// MPD password
    #[arg(short, long)]
    password: Option<String>,

    /// Queue file for offline use
    #[arg(short, long)]
    queue: Option<PathBuf>,

    /// Maximum time between retries
    #[arg(short, long)]
    time: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub mpd_addr: String,
    pub mpd_socket: Option<PathBuf>,
    pub mpd_password: Option<String>,
    pub queue_path: PathBuf,
    pub max_retry_time: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("config error: {0}")]
    Config(#[from] config::ConfigError),

    #[error(transparent)]
    Xdg(#[from] xdg::BaseDirectoriesError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("Invalid UTF-8 in config path")]
    ConfigPath,

    #[error("Invalid UTF-8 in queue file path")]
    QueuePath,
}

impl Settings {
    pub fn new(args: Args) -> Result<Self, SettingsError> {
        let xdg_dirs = xdg::BaseDirectories::with_prefix("scritches")?;
        let mut config_builder = Config::builder()
            .set_default("mpd_addr", "localhost:6600")?
            .set_default(
                "queue_path",
                xdg_dirs
                    .place_state_file("queue")?
                    .to_str()
                    .ok_or(SettingsError::QueuePath)?,
            )?
            .set_default("max_retry_time", 960)?;

        if let Some(addr) = args.addr {
            config_builder = config_builder.set_override("mpd_addr", addr)?;
        }

        if let Some(socket) = args.socket {
            config_builder = config_builder.set_override("mpd_socket", socket)?;
        }

        if let Some(password) = args.password {
            config_builder = config_builder.set_override("mpd_password", password)?;
        }

        if let Some(queue_path) = args.queue {
            config_builder = config_builder.set_override(
                "queue_path",
                queue_path.to_str().ok_or(SettingsError::QueuePath)?,
            )?;
        }

        if let Some(time) = args.time {
            config_builder = config_builder.set_override("max_retry_time", time)?;
        }

        if let Some(config_path) = args.config {
            config_builder = config_builder.add_source(config::File::with_name(
                config_path.to_str().ok_or(SettingsError::ConfigPath)?,
            ));
        } else {
            config_builder = config_builder.add_source(
                config::File::with_name(
                    xdg::BaseDirectories::with_prefix("scritches")?
                        .get_config_file("config")
                        .to_str()
                        .ok_or(SettingsError::ConfigPath)?,
                )
                .required(false),
            );
        }

        Ok(config_builder.build()?.try_deserialize()?)
    }
}
