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
use tokio::{
    net::{TcpStream, UnixStream},
    select,
    sync::mpsc,
};

use std::{
    cmp::min,
    collections::VecDeque,
    fs::File,
    io::{self, Seek},
    path::{Path, PathBuf},
    string::ToString,
    time::Duration,
    time::SystemTime,
};

mod config;
mod last_fm;

use last_fm::{Action, BasicInfo, Client as LastFmClient, Message, ScrobbleInfo};

#[derive(Debug)]
struct WorkQueue {
    scrobble_queue: VecDeque<ScrobbleInfo>,
    action_queue: VecDeque<Action>,
    queue_file: File,
}

impl WorkQueue {
    fn new(path: &Path) -> io::Result<Self> {
        let f = File::open(path)?;
        let (scrobble_queue, action_queue) = bincode::deserialize_from(f).unwrap_or_else(|e| {
            eprintln!("unable to load queue file: {e}");
            (VecDeque::new(), VecDeque::new())
        });

        let queue_file = File::create(path)?;

        Ok(Self {
            scrobble_queue,
            action_queue,
            queue_file,
        })
    }

    pub fn write(&mut self) -> bincode::Result<()> {
        self.queue_file.set_len(0)?;
        self.queue_file.rewind()?;
        bincode::serialize_into(
            &self.queue_file,
            &(&self.scrobble_queue, &self.action_queue),
        )
    }

    pub fn has_work(&self) -> bool {
        !(self.scrobble_queue.is_empty() && self.action_queue.is_empty())
    }

    pub async fn do_work(&mut self, client: &mut LastFmClient) -> bincode::Result<()> {
        println!("scrobbling from queue: {} songs", self.scrobble_queue.len());
        while let Some(info) = self.scrobble_queue.front() {
            match client.scrobble_one(info).await {
                Ok(_) => {
                    self.scrobble_queue.pop_front();
                }
                Err(_) => {
                    eprintln!("client lost connection...");
                    println!("{} songs still in queue", self.scrobble_queue.len());
                    break;
                }
            }
        }
        self.write()?;
        Ok(())
    }

    pub fn add_scrobble(&mut self, info: ScrobbleInfo) -> bincode::Result<()> {
        self.scrobble_queue.push_back(info);
        self.write()
    }

    pub fn add_action(&mut self, action: Action) -> bincode::Result<()> {
        self.action_queue.push_back(action);
        self.write()
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

    let mut work_queue = WorkQueue::new(&settings.queue_path)?;
    let mut last_fm_client = LastFmClient::new();
    // write queue out immediately to avoid empty queue file
    work_queue.write()?;

    work_queue.do_work(&mut last_fm_client).await?;

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            let r = rx.recv();
            if work_queue.has_work() {
                let t = interval.tick();
                select! {
                    Some(msg) = r => {
                        match msg {
                            Message::Scrobble(info) => work_queue.add_scrobble(info).expect("aaaaa"),
                            Message::Action(action) => work_queue.add_action(action).expect("aaaaa"),
                            _ => {}
                        }
                        work_queue.do_work(&mut last_fm_client).await
                    },
                    _ = t => work_queue.do_work(&mut last_fm_client).await,
                    else => break,
                }.expect("aaaaa");
            } else if let Some(msg) = r.await {
                match msg {
                    Message::Scrobble(info) => {
                        if last_fm_client.scrobble_one(&info).await.is_err() {
                            work_queue.add_scrobble(info).expect("aaaaa");
                        }
                    }
                    Message::Action(action) => work_queue.add_action(action).expect("aaaaa"),
                    _ => {}
                }
            } else {
                break;
            }
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
                    handle_mpd_msg(&client, &tx, song).await?
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
                tx.send(Message::NowPlaying(info)).await?;
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

async fn handle_mpd_msg(
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

async fn handle_msg() {}

#[inline]
fn check_scrobble(start: Duration, cur: Duration, length: Duration) -> bool {
    true
    // (cur - start) >= min(Duration::from_secs(240), length / 2) && length > Duration::from_secs(30)
}

fn scrobble_info(song: &Song, start_time: SystemTime) -> Result<ScrobbleInfo, SongError> {
    let title = song.title().ok_or(SongError::NoTitle)?.to_string();
    let artist = (!song.artists().is_empty())
        .then(|| song.artists().join(", "))
        .ok_or(SongError::NoArtist)?;
    let album = song.album().map(ToString::to_string);
    let album_artist = (!song.album_artists().is_empty())
        .then(|| song.album_artists().join(", "))
        .filter(|a| a.ne(&artist));
    let track_id = song
        .tags
        .get(&Tag::MusicBrainzRecordingId)
        .and_then(|t| t.first())
        .cloned();
    Ok(ScrobbleInfo {
        title,
        artist,
        album,
        album_artist,
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
