use config::Config;
use serde_derive::Deserialize;
use thiserror::Error;

use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub mpd_host: String,
    pub mpd_port: u16,
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error(transparent)]
    ConfigError(#[from] config::ConfigError),

    #[error(transparent)]
    XDGError(#[from] xdg::BaseDirectoriesError),
}

impl Settings {
    pub fn new(
        host: Option<String>,
        port: Option<u16>,
        config: Option<PathBuf>,
    ) -> std::result::Result<Self, SettingsError> {
        let mut config_builder = Config::builder()
            .set_default("mpd_host", "localhost")?
            .set_default("mpd_port", 6600)?;

        if let Some(host) = host {
            config_builder = config_builder.set_override("mpd_host", host)?;
        }

        if let Some(port) = port {
            config_builder = config_builder.set_override("mpd_port", port)?;
        }

        if let Some(config_path) = config {
            config_builder = config_builder.add_source(config::File::with_name(
                config_path.to_str().expect("invalid UTF-8 in config path"),
            ));
        } else {
            config_builder = config_builder.add_source(
                config::File::with_name(
                    xdg::BaseDirectories::with_prefix("scritches")?
                        .get_config_file("config")
                        .to_str()
                        .expect("invalid UTF-8 in config path"),
                )
                .required(false),
            );
        }

        config_builder = config_builder.add_source(config::Environment::default());

        Ok(config_builder.build()?.try_deserialize()?)
    }
}
