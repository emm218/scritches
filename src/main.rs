use clap::Parser;
use config::Config;
use serde_derive::Deserialize;

use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// detach from current terminal after startup
    #[arg(short, long)]
    detach: bool,

    /// hostname for MPD
    #[arg(short = 'H', long)]
    host: Option<String>,

    /// port for MPD
    #[arg(short, long)]
    port: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct Settings {
    mpd_host: String,
    mpd_port: u16,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let mut config_builder = Config::builder()
        .set_default("mpd_host", "localhost")?
        .set_default("mpd_port", 6600)?;

    if let Some(host) = args.host {
        config_builder = config_builder.set_override("mpd_host", host)?;
    }

    if let Some(port) = args.port {
        config_builder = config_builder.set_override("mpd_port", port)?;
    }

    if let Some(config_path) = args.config {
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

    let settings: Settings = config_builder.build()?.try_deserialize()?;

    println!("{}:{}", settings.mpd_host, settings.mpd_port);

    Ok(())
}
