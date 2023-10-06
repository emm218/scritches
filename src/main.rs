#![feature(let_chains)]
#![feature(duration_constants)]

use anyhow::anyhow;
use clap::Parser;
use mpd_client::{
    client::{ConnectionEvent, Subsystem},
    commands,
    responses::Song,
    Client as MpdClient,
};
use tokio::net::TcpStream;

use std::cmp::min;
use std::path::PathBuf;
use std::time::Duration;

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

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let settings = config::Settings::new(args.host, args.port, args.config)?;

    let conn = TcpStream::connect(&format!("{}:{}", settings.mpd_host, settings.mpd_port)).await?;

    let (client, mut state_changes) = MpdClient::connect(conn).await?;

    let stats = client.command(commands::Stats).await?;
    let status = client.command(commands::Status).await?;

    let elapsed = status.elapsed.unwrap_or_default();
    let mut length = status.duration.unwrap_or_default();
    let mut start = stats.playtime - elapsed;
    let mut song_in_queue = client.command(commands::CurrentSong).await?;

    'outer: loop {
        loop {
            match state_changes.next().await {
                Some(ConnectionEvent::SubsystemChange(Subsystem::Player)) => break,
                Some(ConnectionEvent::SubsystemChange(_)) => continue,
                _ => break 'outer,
            }
        }

        let stats = client.command(commands::Stats).await?;
        let status = client.command(commands::Status).await?;

        let elapsed = status.elapsed.unwrap_or_default();
        let cur_time = stats.playtime;

        match (
            &song_in_queue,
            status.current_song.map(|s| s.1).zip(status.duration),
        ) {
            (Some(song), Some((id, _))) if song.id == id => {
                if check_submit(start, cur_time, length) && elapsed < Duration::from_secs(1) {
                    submit_song(&song.song);
                    start = cur_time;
                }
            }

            (old, new) => {
                if check_submit(start, cur_time, length) && let Some(song) = old {
                submit_song(&song.song);
            }
                start = cur_time;
                length = new.map_or(length, |s| s.1);
                song_in_queue = client.command(commands::CurrentSong).await?
            }
        }
    }
    Err(anyhow!("Connection closed by server"))
}

#[inline]
fn check_submit(start: Duration, cur: Duration, length: Duration) -> bool {
    (cur - start) >= min(Duration::from_secs(240), length / 2)
}

fn submit_song(song: &Song) -> () {
    println!(
        "{} - {}",
        song.artists().join(", "),
        song.title().unwrap_or(""),
    );
}
