#![feature(let_chains)]
#![feature(iter_intersperse)]
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
    string::ToString,
    time::SystemTime,
    time::{Duration, UNIX_EPOCH},
};

mod last_fm;
mod settings;
mod work_queue;

use crate::{
    last_fm::{BasicInfo, Client as LastFmClient, Message, ScrobbleInfo, SongInfo},
    settings::Args,
    work_queue::{Error as WorkError, WorkQueue},
};

#[derive(Debug, thiserror::Error)]
enum SongError {
    #[error("title is missing")]
    NoTitle,
    #[error("artist is missing")]
    NoArtist,
}

#[derive(Debug, thiserror::Error)]
enum MsgHandleError {
    #[error("channel closed")]
    ChannelClosed,

    #[error(transparent)]
    BinCode(#[from] bincode::Error),
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

    let settings = settings::Settings::new(args)?;

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

    //TODO: we should be able to start adding stuff to the work queue while waiting for this future
    //to finish but that will require some substantial architecture change
    //
    //look into how to check if a future is done and do different actions based on that
    let mut last_fm_client = LastFmClient::new().await?;

    if work_queue.has_work() {
        if let Err(WorkError::BinCode(e)) = work_queue.do_work(&mut last_fm_client).await {
            panic!("{e}");
        };
    }

    let max_retry_time = Duration::from_secs(settings.max_retry_time);

    tokio::spawn(async move {
        let mut retry_time = Duration::from_secs(15);

        loop {
            retry_time = min(max_retry_time, retry_time);

            retry_time =
                match handle_async_msg(&mut rx, retry_time, &mut work_queue, &mut last_fm_client)
                    .await
                {
                    Ok(t) => t,
                    Err(MsgHandleError::ChannelClosed) => break,
                    Err(MsgHandleError::BinCode(e)) => panic!("{e}"),
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

    let mut start_time = SystemTime::now().duration_since(UNIX_EPOCH)?;

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
                .await?;
            }
            Some(ConnectionEvent::SubsystemChange(Subsystem::Message)) => {
                if let Some(song) = current_song.as_ref() {
                    handle_mpd_msg(&client, &tx, song).await?;
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
    start_time: Duration,
    current_song: Option<SongInQueue>,
) -> anyhow::Result<(Duration, Duration, Duration, Option<SongInQueue>)> {
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
                Ok((
                    length,
                    cur_playtime,
                    SystemTime::now().duration_since(UNIX_EPOCH)?,
                    current_song,
                ))
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
            if let Some(Ok(info)) = new_song.as_ref().map(|s| song_info(&s.song)) {
                tx.send(Message::NowPlaying(info)).await?;
            }
            Ok((
                new.map_or(length, |s| s.1),
                cur_playtime,
                SystemTime::now().duration_since(UNIX_EPOCH)?,
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

    let mut love = true;

    for m in messages {
        if m.1 == "love" {
            love = true;
        } else if m.1 == "unlove" {
            love = false;
        }
    }

    if love {
        tx.send(Message::love_track(info)).await?;
    } else {
        tx.send(Message::unlove_track(info)).await?;
    }

    Ok(())
}

async fn handle_async_msg(
    rx: &mut mpsc::Receiver<Message>,
    retry_time: Duration,
    work_queue: &mut WorkQueue,
    client: &mut LastFmClient,
) -> Result<Duration, MsgHandleError> {
    let r = rx.recv();
    let t = tokio::time::sleep(retry_time);

    if work_queue.has_work() {
        select! {
            Some(msg) = r => {
                match msg {
                    Message::Scrobble(info) => work_queue.add_scrobble(info)?,
                    Message::Action(action) => work_queue.add_action(action)?,
                    _ => { return Ok(retry_time); }
                };
                match work_queue.do_work(client).await {
                    Ok(_) => Ok(Duration::from_secs(15)),
                    Err(WorkError::BinCode(e)) => Err(e.into()),
                    _ => Ok(retry_time),
                }
            },
            () = t => match work_queue.do_work(client).await {
                    Ok(_) => Ok(Duration::from_secs(15)),
                    Err(WorkError::LastFm(_)) => Ok(retry_time * 2),
                    Err(WorkError::BinCode(e)) => Err(e.into()),
                },
            else => Err(MsgHandleError::ChannelClosed),
        }
    } else if let Some(msg) = r.await {
        match msg {
            Message::Scrobble(info) => {
                if let Err(e) = client.scrobble_one(&info).await {
                    eprintln!("{e}");
                    work_queue.add_scrobble(info)?;
                }
            }
            Message::Action(action) => {
                work_queue.add_action(action)?;
            }
            Message::NowPlaying(info) => {
                let _ = client.now_playing(&info).await;
            }
        }
        Ok(Duration::from_secs(15))
    } else {
        Err(MsgHandleError::ChannelClosed)
    }
}

#[inline]
fn check_scrobble(start: Duration, cur: Duration, length: Duration) -> bool {
    (cur - start) >= min(Duration::from_secs(240), length / 2) && length > Duration::from_secs(30)
}

fn scrobble_info(song: &Song, start_time: Duration) -> Result<ScrobbleInfo, SongError> {
    let song = song_info(song)?;
    Ok(ScrobbleInfo {
        song,
        timestamp: start_time.as_secs().to_string(),
    })
}

fn song_info(song: &Song) -> Result<SongInfo, SongError> {
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
    Ok(SongInfo {
        title,
        artist,
        album,
        album_artist,
        track_id,
    })
}

fn basic_info(song: &Song) -> Result<BasicInfo, SongError> {
    let title = song.title().ok_or(SongError::NoTitle)?.to_string();
    let artist = (!song.artists().is_empty())
        .then(|| song.artists().join(", "))
        .ok_or(SongError::NoArtist)?;
    Ok(BasicInfo { title, artist })
}
