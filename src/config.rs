use config::Config;
use serde_derive::Deserialize;

use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub mpd_addr: String,
    pub mpd_socket: Option<PathBuf>,
    pub mpd_password: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    ConfigError(#[from] config::ConfigError),

    #[error(transparent)]
    XDGError(#[from] xdg::BaseDirectoriesError),
}

impl Settings {
    pub fn new(
        addr: Option<String>,
        socket: Option<String>,
        password: Option<String>,
        config: Option<PathBuf>,
    ) -> std::result::Result<Self, SettingsError> {
        let mut config_builder = Config::builder().set_default("mpd_addr", "localhost:6600")?;

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
