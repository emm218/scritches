#![feature(let_chains)]
#![feature(duration_constants)]

use anyhow::anyhow;
use clap::Parser;
use mpd_client::{
    client::{ConnectWithPasswordError, Connection, ConnectionEvent, Subsystem},
    commands,
    responses::Song,
    tag::Tag,
    Client as MpdClient,
};
use tokio::{
    net::{TcpStream, UnixStream},
    sync::mpsc,
};

use std::cmp::min;
use std::path::PathBuf;
use std::time::Duration;

mod config;
mod last_fm;

use last_fm::SongInfo;

#[derive(Debug, Parser)]
#[command(version)]
struct Args {
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
}

enum Connector {
    Tcp(TcpStream),
    Uds(UnixStream),
}

impl Connector {
    pub async fn connect(
        self,
        password: Option<&str>,
    ) -> Result<Connection, ConnectWithPasswordError> {
        match self {
            Self::Tcp(stream) => MpdClient::connect_with_password_opt(stream, password).await,
            Self::Uds(stream) => MpdClient::connect_with_password_opt(stream, password).await,
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let settings = config::Settings::new(args.addr, args.socket, args.password, args.config)?;

    let conn: Connector = if let Some(sock) = settings.mpd_socket {
        println!("connecting to MPD at {}", sock.display());
        match UnixStream::connect(&sock).await {
            Ok(sock) => Connector::Uds(sock),
            Err(e) => {
                println!(
                    "failed to connect to unix socket `{}`: {e}\ntrying TCP at {}...",
                    sock.display(),
                    settings.mpd_addr
                );
                Connector::Tcp(TcpStream::connect(&settings.mpd_addr).await?)
            }
        }
    } else {
        println!("connecting to MPD at {}", settings.mpd_addr);
        Connector::Tcp(TcpStream::connect(&settings.mpd_addr).await?)
    };

    let (client, mut state_changes) = conn.connect(settings.mpd_password.as_deref()).await?;

    println!("connected!");

    let (tx, mut rx) = mpsc::channel(5);

    tokio::spawn(async move {
        while let Some(info) = rx.recv().await {
            println!("{info:?}");
        }
    });

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
                if check_scrobble(start, cur_time, length) && elapsed < Duration::from_secs(1) {
                    match song_info(&song.song) {
                        Err(e) => eprintln!("can't scrobble song: {e}"),
                        Ok(info) => tx.send(info).await?,
                    }
                    start = cur_time;
                }
            }

            (old, new) => {
                if check_scrobble(start, cur_time, length) && let Some(song) = old {
                    match song_info(&song.song) {
                        Err(e) => eprintln!("can't scrobble song: {e}"),
                        Ok(info) => tx.send(info).await?,
                    }
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
fn check_scrobble(start: Duration, cur: Duration, length: Duration) -> bool {
    (cur - start) >= min(Duration::from_secs(240), length / 2)
}

#[derive(Debug, thiserror::Error)]
enum SongError {
    #[error("title is missing")]
    NoTitle,
    #[error("artist is missing")]
    NoArtist,
}

fn song_info(song: &Song) -> Result<SongInfo, SongError> {
    let title = song.title().ok_or(SongError::NoTitle)?.to_string();
    let artist = (!song.artists().is_empty())
        .then(|| song.artists().join(", "))
        .ok_or(SongError::NoArtist)?;
    let album = song.album().map(|a| a.to_string());
    let album_artist = (!song.album_artists().is_empty())
        .then(|| song.artists().join(", "))
        .filter(|a| a.ne(&artist));
    let track_id = song
        .tags
        .get(&Tag::MusicBrainzTrackId)
        .map(|t| t.first())
        .flatten()
        .cloned();
    Ok(SongInfo {
        title,
        artist,
        album_artist,
        album,
        track_id,
    })
}
