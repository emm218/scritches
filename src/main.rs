#![feature(let_chains)]
#![feature(duration_constants)]

use clap::Parser;
use mpd::idle::{Idle, Subsystem};
use mpd::song::Id as SongId;

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

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let settings = config::Settings::new(args.host, args.port, args.config)?;

    let mut conn = mpd::Client::connect(&format!("{}:{}", settings.mpd_host, settings.mpd_port))?;

    let stats = conn.stats()?;
    let status = conn.status()?;

    let elapsed = status.elapsed.unwrap_or_default();
    let length = status.duration.unwrap_or_default();

    conn.wait(&[Subsystem::Player])?;

    next_event(
        stats.playtime - elapsed,
        length,
        elapsed,
        status.song.map(|s| s.id),
        &mut conn,
    )
}

fn next_event(
    prev_start: Duration,
    prev_length: Duration,
    prev_elapsed: Duration,
    prev_song: Option<mpd::song::Id>,
    conn: &mut mpd::Client,
) -> anyhow::Result<()> {
    let status = conn.status()?;
    let cur_time = conn.stats()?.playtime;

    let elapsed = status.elapsed.unwrap_or(prev_elapsed);

    let (start, length, song) = match (prev_song, status.song.map(|s| s.id).zip(status.duration)) {
        (Some(id), Some((id2, _))) if id == id2 => {
            let t = if check_submit(prev_start, cur_time, prev_length)
                && elapsed < Duration::from_secs(1)
            {
                submit_song(conn, id)?;
                cur_time
            } else {
                prev_start
            };
            (t, prev_length, prev_song)
        }

        (old, new) => {
            if check_submit(prev_start, cur_time, prev_length) && let Some(id) = old {
                submit_song(conn, id)?;
            }
            (cur_time, new.map_or(prev_length, |s| s.1), new.map(|s| s.0))
        }
    };

    conn.wait(&[Subsystem::Player])?;
    next_event(start, length, elapsed, song, conn)
}

#[inline]
fn check_submit(start: Duration, cur: Duration, length: Duration) -> bool {
    (cur - start) >= min(Duration::from_secs(240), length / 2)
}

#[derive(Debug, thiserror::Error)]
enum SongSubmitError {
    #[error(transparent)]
    MpdError(#[from] mpd::error::Error),

    #[error("id {0} not found in playlist")]
    IdError(SongId),
}

fn submit_song(conn: &mut mpd::Client, id: SongId) -> Result<(), SongSubmitError> {
    let song = conn.playlistid(id)?.ok_or(SongSubmitError::IdError(id))?;
    println!(
        "{} - {}",
        song.artist.unwrap_or_default(),
        song.title.unwrap_or_default()
    );
    Ok(())
}
