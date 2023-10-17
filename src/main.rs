#![feature(let_chains)]
#![feature(duration_constants)]

use anyhow::anyhow;
use clap::Parser;
use mpd_client::{
    client::{ConnectWithPasswordError, Connection, ConnectionEvent, Subsystem},
    commands::{CurrentSong, ReadChannelMessages, Stats, Status, SubscribeToChannel},
    responses::{Song, SongInQueue},
    tag::Tag,
    Client as MpdClient,
};
use serde_derive::{Deserialize, Serialize};
use tokio::{
    net::{TcpStream, UnixStream},
    sync::mpsc,
};

use std::{
    cmp::min, collections::VecDeque, fs::File, io::Seek, path::PathBuf, time::Duration,
    time::SystemTime,
};

mod config;
mod last_fm;

use last_fm::{Action, BasicInfo, Message, ScrobbleInfo};

#[derive(Debug, Serialize, Deserialize)]
struct WorkQueue {
    pub scrobble_queue: Vec<ScrobbleInfo>,
    pub action_queue: VecDeque<Action>,
}

impl WorkQueue {
    fn new() -> Self {
        Self {
            scrobble_queue: Vec::new(),
            action_queue: VecDeque::new(),
        }
    }

    pub fn write_queue(&self, queue_file: &mut File) -> bincode::Result<()> {
        queue_file.set_len(0)?;
        queue_file.rewind()?;
        bincode::serialize_into(queue_file, self)
    }
}

#[derive(Debug, thiserror::Error)]
enum SongError {
    #[error("title is missing")]
    NoTitle,
    #[error("artist is missing")]
    NoArtist,
}

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

    /// Queue file for offline use
    #[arg(short, long)]
    queue: Option<PathBuf>,
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

    let settings = config::Settings::new(
        args.addr,
        args.socket,
        args.password,
        args.config,
        args.queue,
    )?;

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

    let mut work_queue = match File::open(&settings.queue_path) {
        Ok(f) => match bincode::deserialize_from(f) {
            Ok(wq) => wq,
            Err(e) => {
                eprintln!("unable to load queue file: {e}");
                WorkQueue::new()
            }
        },
        Err(_) => WorkQueue::new(),
    };

    let mut queue_file = File::create(&settings.queue_path)?;
    work_queue.write_queue(&mut queue_file)?;

    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            println!("{msg:?}");
            match msg {
                Message::Scrobble(info) => work_queue.scrobble_queue.push(info),
                Message::Action(action) => work_queue.action_queue.push_back(action),
                Message::NowPlaying { .. } => {}
            }
            work_queue.write_queue(&mut queue_file).expect("aaaaa");
        }
    });

    client.command(SubscribeToChannel("scritches")).await?;

    let stats = client.command(Stats).await?;
    let status = client.command(Status).await?;

    let elapsed = status.elapsed.unwrap_or_default();
    let mut length = status.duration.unwrap_or_default();
    let mut start_playtime = stats.playtime - elapsed;
    let mut current_song = client.command(CurrentSong).await?;

    let mut start_time = SystemTime::now();

    loop {
        match state_changes.next().await {
            Some(ConnectionEvent::SubsystemChange(Subsystem::Player)) => {
                (length, start_playtime, start_time, current_song) = handle_player(
                    &client,
                    &tx,
                    length,
                    start_playtime,
                    start_time,
                    current_song,
                )
                .await?
            }
            Some(ConnectionEvent::SubsystemChange(Subsystem::Message)) => {
                if let Some(song) = current_song.as_ref() {
                    handle_message(&client, &tx, song).await?
                }
            }
            Some(ConnectionEvent::SubsystemChange(_)) => continue,
            _ => break,
        }
    }
    Err(anyhow!("Connection closed by server"))
}

async fn handle_player(
    client: &MpdClient,
    tx: &mpsc::Sender<Message>,
    length: Duration,
    start_playtime: Duration,
    start_time: SystemTime,
    current_song: Option<SongInQueue>,
) -> anyhow::Result<(Duration, Duration, SystemTime, Option<SongInQueue>)> {
    let stats = client.command(Stats).await?;
    let status = client.command(Status).await?;

    let elapsed = status.elapsed.unwrap_or_default();
    let cur_playtime = stats.playtime;

    match (
        &current_song,
        status.current_song.map(|s| s.1).zip(status.duration),
    ) {
        (Some(song), Some((id, _))) if song.id == id => {
            if check_scrobble(start_playtime, cur_playtime, length)
                && elapsed < Duration::from_secs(1)
            {
                match scrobble_info(&song.song, start_time) {
                    Err(e) => eprintln!("can't scrobble song: {e}"),
                    Ok(info) => tx.send(Message::Scrobble(info)).await?,
                }
                Ok((length, cur_playtime, SystemTime::now(), current_song))
            } else {
                Ok((length, start_playtime, start_time, current_song))
            }
        }

        (old, new) => {
            if check_scrobble(start_playtime, cur_playtime, length) && let Some(song) = old {
                    match scrobble_info(&song.song, start_time) {
                        Err(e) => eprintln!("can't scrobble song: {e}"),
                        Ok(info) => tx.send(Message::Scrobble(info)).await?,
                    }
                }
            let new_song = client.command(CurrentSong).await?;
            if let Some(Ok(info)) = new_song.as_ref().map(|s| basic_info(&s.song)) {
                tx.send(Message::now_playing(info)).await?;
            }
            Ok((
                new.map_or(length, |s| s.1),
                cur_playtime,
                SystemTime::now(),
                new_song,
            ))
        }
    }
}

async fn handle_message(
    client: &MpdClient,
    tx: &mpsc::Sender<Message>,
    current_song: &SongInQueue,
) -> anyhow::Result<()> {
    let info = basic_info(&current_song.song)?;

    let messages = client.command(ReadChannelMessages).await?;
    for m in messages {
        if m.1 == "love" {
            // clone here is evil because we have 2 strings :( but we can't use Arc instead because
            // it doesn't play nice with serde...oh no
            tx.send(Message::love_track(info.clone())).await?;
        } else if m.1 == "unlove" {
            tx.send(Message::unlove_track(info.clone())).await?;
        }
    }
    Ok(())
}

#[inline]
fn check_scrobble(start: Duration, cur: Duration, length: Duration) -> bool {
    (cur - start) >= min(Duration::from_secs(240), length / 2) && length > Duration::from_secs(30)
}

fn scrobble_info(song: &Song, start_time: SystemTime) -> Result<ScrobbleInfo, SongError> {
    let title = song.title().ok_or(SongError::NoTitle)?.to_string();
    let artist = (!song.artists().is_empty())
        .then(|| song.artists().join(", "))
        .ok_or(SongError::NoArtist)?;
    let album = song.album().map(|a| a.to_string());
    let album_artist = (!song.album_artists().is_empty())
        .then(|| song.album_artists().join(", "))
        .filter(|a| a.ne(&artist));
    let track_id = song
        .tags
        .get(&Tag::MusicBrainzRecordingId)
        .map(|t| t.first())
        .flatten()
        .cloned();
    Ok(ScrobbleInfo {
        title,
        artist,
        album_artist,
        album,
        track_id,
        start_time,
    })
}

fn basic_info(song: &Song) -> Result<BasicInfo, SongError> {
    let title = song.title().ok_or(SongError::NoTitle)?.to_string();
    let artist = (!song.artists().is_empty())
        .then(|| song.artists().join(", "))
        .ok_or(SongError::NoArtist)?;
    Ok(BasicInfo { title, artist })
}
