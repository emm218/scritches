#![feature(let_chains)]
#![feature(iter_intersperse)]
#![feature(duration_constants)]

use anyhow::anyhow;
use clap::Parser;
use last_fm::{Action, BasicInfo, SongInfo};
use log::{error, info, warn};
use mpd_client::{
    client::{ConnectWithPasswordError, Connection, ConnectionEvent, Subsystem},
    commands::{CurrentSong, ReadChannelMessages, Stats, Status, SubscribeToChannel},
    responses::SongInQueue,
    Client as MpdClient,
};
use serde_derive::{Deserialize, Serialize};
use tokio::{
    net::{TcpStream, UnixStream},
    select,
    sync::mpsc,
};

use std::{
    cmp::min,
    time::SystemTime,
    time::{Duration, UNIX_EPOCH},
};

mod last_fm;
mod settings;
mod work_queue;

use crate::{
    last_fm::Client as LastFmClient,
    settings::Args,
    work_queue::{Error as WorkError, WorkQueue},
};

#[derive(Debug, thiserror::Error)]
enum MsgHandleError {
    #[error("channel closed")]
    ChannelClosed,

    #[error(transparent)]
    BinCode(#[from] bincode::Error),
}

#[derive(Debug, Serialize, Deserialize)]
enum Message {
    Scrobble(SongInfo, String),
    NowPlaying(SongInfo),
    TrackAction(Action, BasicInfo),
}

impl Message {
    pub fn love_track(info: BasicInfo) -> Self {
        Message::TrackAction(Action::Love, info)
    }

    pub fn unlove_track(info: BasicInfo) -> Self {
        Message::TrackAction(Action::Unlove, info)
    }
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
    env_logger::init();

    let args = Args::parse();

    let settings = settings::Settings::new(args)?;

    let conn: Connector = if let Some(sock) = settings.mpd_socket {
        info!("connecting to MPD at {}", sock.display());
        match UnixStream::connect(&sock).await {
            Ok(sock) => Connector::Uds(sock),
            Err(e) => {
                warn!("failed to connect to unix socket `{}`: {e}", sock.display(),);
                info!("connecting to MPD at {}", settings.mpd_addr);
                Connector::Tcp(TcpStream::connect(&settings.mpd_addr).await?)
            }
        }
    } else {
        info!("connecting to MPD at {}", settings.mpd_addr);
        Connector::Tcp(TcpStream::connect(&settings.mpd_addr).await?)
    };

    let (client, mut state_changes) = conn.connect(settings.mpd_password.as_deref()).await?;

    info!("connected!");

    let (tx, mut rx) = mpsc::channel(5);

    let mut work_queue = WorkQueue::new(&settings.queue_path)?;

    //TODO: we should be able to start adding stuff to the work queue while waiting for this future
    //to finish but that will require some substantial architecture change
    //
    //look into how to check if a future is done and do different actions based on that
    //
    //edit: oh god its the revenge of polling
    //
    //plan: 2 loops in async task, 1 that selects on this future and receiving a message, breaking
    //when this future is done, the other that acts as we currently have it
    let mut last_fm_client = LastFmClient::new(settings.sk_path).await?;

    if work_queue.has_work() {
        if let Err(WorkError::BinCode(e)) = work_queue.do_work(&mut last_fm_client).await {
            panic!("{e}");
        };
    }

    let max_retry_time = Duration::from_secs(settings.max_retry_time);

    //TODO: more graceful shutdown
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
                if let Ok(info) = song.try_into() {
                    tx.send(Message::NowPlaying(info)).await?;
                }
                match song.try_into() {
                    Err(e) => warn!("couldn't scrobble song: {e}"),
                    Ok(info) => {
                        let timestamp = start_time.as_secs().to_string();
                        tx.send(Message::Scrobble(info, timestamp)).await?;
                    }
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
                    match song.try_into() {
                        Err(e) => warn!("couldn't scrobble song: {e}"),
                        Ok(info) => {
                            let timestamp = start_time.as_secs().to_string();
                            tx.send(Message::Scrobble(info, timestamp)).await?;
                        }
                    }
                }
            let new_song = client.command(CurrentSong).await?;
            if let Some(Ok(info)) = new_song.as_ref().map(TryInto::try_into) {
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
    let info = current_song.try_into()?;

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
                    Message::Scrobble(info, timestamp) => work_queue.add_scrobble(info, timestamp)?,
                    Message::TrackAction(action, info) => work_queue.add_action(action, info)?,
                    Message::NowPlaying(_) => { return Ok(retry_time); }
                };
                match work_queue.do_work(client).await {
                    Ok(_) => Ok(Duration::from_secs(15)),
                    Err(WorkError::LastFm(_)) => Ok(retry_time),
                    Err(WorkError::BinCode(e)) => Err(e.into()),
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
            Message::Scrobble(info, timestamp) => {
                info!("scrobbling {} - {}", info.artist, info.title);
                if let Err(e) = client.scrobble_one(&info, &timestamp).await {
                    warn!("scrobble failed: {e}");
                    work_queue.add_scrobble(info, timestamp)?;
                } else {
                    info!("scrobbled successfully")
                }
            }
            Message::TrackAction(action, info) => {
                info!("{}ing {} - {}", action, info.artist, info.title);
                if let Err(e) = client.do_track_action(action, &info).await {
                    warn!("action failed: {e}");
                    work_queue.add_action(action, info)?;
                } else {
                    info!("{}ed successfully", action);
                }
            }
            Message::NowPlaying(info) => {
                if let Ok(()) = client.now_playing(&info).await {
                    info!("updated now playing status successfully")
                }
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
