use clap::Parser;
use config::Config;
use serde::Deserialize;

use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(version)]
pub struct Args {
    /// Path to config file
    #[arg(short, long)]
    config: Option<String>,

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
    queue: Option<String>,

    /// Session key file
    #[arg(short, long)]
    key: Option<String>,

    /// Maximum time between retries
    #[arg(short, long)]
    time: Option<u64>,

    /// Exit program if user needs to (re)authorize
    #[arg(short = 'i', long)]
    pub non_interactive: bool,
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub mpd_addr: String,
    pub mpd_socket: Option<PathBuf>,
    pub mpd_password: Option<String>,
    pub queue_path: PathBuf,
    pub sk_path: PathBuf,
    pub max_retry_time: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
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

    #[error("Ivalid UTF-8 in session key file path")]
    KeyPath,
}

impl Settings {
    pub fn new(args: Args) -> Result<Self, Error> {
        let xdg_dirs = xdg::BaseDirectories::with_prefix("scritches")?;
        let mut config_builder = Config::builder()
            .set_default("mpd_addr", "localhost:6600")?
            .set_default(
                "queue_path",
                xdg_dirs
                    .place_state_file("queue")?
                    .to_str()
                    .ok_or(Error::QueuePath)?,
            )?
            .set_default(
                "sk_path",
                xdg_dirs
                    .place_state_file("sk")?
                    .to_str()
                    .ok_or(Error::KeyPath)?,
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
            config_builder = config_builder.set_override("queue_path", queue_path)?;
        }

        if let Some(sk_path) = args.key {
            config_builder = config_builder.set_override("sk_path", sk_path)?;
        }

        if let Some(time) = args.time {
            config_builder = config_builder.set_override("max_retry_time", time)?;
        }

        config_builder = config_builder.add_source(if let Some(config_path) = args.config {
            config::File::with_name(&config_path)
        } else {
            config::File::with_name(
                xdg_dirs
                    .get_config_file("config")
                    .to_str()
                    .ok_or(Error::ConfigPath)?,
            )
            .required(false)
        });

        Ok(config_builder.build()?.try_deserialize()?)
    }
}
