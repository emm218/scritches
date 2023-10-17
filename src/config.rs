use config::Config;
use serde_derive::Deserialize;

use std::{
    path::PathBuf,
    result::{self, Result},
};

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub mpd_addr: String,
    pub mpd_socket: Option<PathBuf>,
    pub mpd_password: Option<String>,
    pub queue_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("config error: {0}")]
    ConfigError(#[from] config::ConfigError),

    #[error(transparent)]
    XDGError(#[from] xdg::BaseDirectoriesError),

    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error("Invalid UTF-8 in config path")]
    ConfigPathError,

    #[error("Invalid UTF-8 in queue file path")]
    QueuePathError,
}

impl Settings {
    pub fn new(
        addr: Option<String>,
        socket: Option<String>,
        password: Option<String>,
        config: Option<PathBuf>,
        queue: Option<PathBuf>,
    ) -> Result<Self, SettingsError> {
        let xdg_dirs = xdg::BaseDirectories::with_prefix("scritches")?;
        let mut config_builder = Config::builder()
            .set_default("mpd_addr", "localhost:6600")?
            .set_default(
                "queue_path",
                xdg_dirs
                    .place_state_file("queue")?
                    .to_str()
                    .ok_or(SettingsError::QueuePathError)?,
            )?;

        if let Some(addr) = addr {
            config_builder = config_builder.set_override("mpd_addr", addr)?;
        }

        if let Some(socket) = socket {
            config_builder = config_builder.set_override("mpd_socket", socket)?;
        }

        if let Some(password) = password {
            config_builder = config_builder.set_override("mpd_password", password)?;
        }

        if let Some(config_path) = config {
            config_builder = config_builder.add_source(config::File::with_name(
                config_path.to_str().ok_or(SettingsError::ConfigPathError)?,
            ));
        } else {
            config_builder = config_builder.add_source(
                config::File::with_name(
                    xdg::BaseDirectories::with_prefix("scritches")?
                        .get_config_file("config")
                        .to_str()
                        .ok_or(SettingsError::ConfigPathError)?,
                )
                .required(false),
            );
        }

        if let Some(queue_path) = queue {
            config_builder = config_builder.set_override("queue_path", queue_path.to_str())?;
        }

        Ok(config_builder.build()?.try_deserialize()?)
    }
}
