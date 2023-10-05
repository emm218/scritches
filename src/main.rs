use clap::Parser;

use std::path::PathBuf;

use mpd::idle::{Idle, Subsystem};

mod config;

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

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let settings = config::Settings::new(args.host, args.port, args.config)?;

    let mut conn = mpd::Client::connect(&format!("{}:{}", settings.mpd_host, settings.mpd_port))?;

    loop {
        println!("Status: {:?}", conn.status());

        conn.wait(&[Subsystem::Player])?;
    }
}
