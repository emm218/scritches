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
use tokio_util::sync::CancellationToken;

use std::{
    cmp::min,
    path::Path,
    time::SystemTime,
    time::{Duration, UNIX_EPOCH},
};

mod last_fm;
mod settings;
mod work_queue;

use crate::{
    last_fm::{Client as LastFmClient, Error as LastFmError},
    settings::Args,
    work_queue::WorkQueue,
};

#[derive(Debug, thiserror::Error)]
enum MsgHandleError {
    #[error("channel closed")]
    ChannelClosed,

    /// unrecoverable API errors
    #[error(transparent)]
    LastFmFatal(LastFmError),

    #[error(transparent)]
    LastFmReauth(LastFmError),
}

impl From<LastFmError> for MsgHandleError {
    fn from(e: LastFmError) -> Self {
        match e {
            LastFmError::ApiReauth(_, _) => Self::LastFmReauth(e),
            _ => Self::LastFmFatal(e),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum Message {
    Scrobble(SongInfo, String),
    NowPlaying(Option<SongInfo>),
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
async fn main() {
    env_logger::builder().format_timestamp(None).init();

    if let Err(e) = main_inner().await {
        error!("{e}");
        std::process::exit(1);
    }
}

async fn main_inner() -> anyhow::Result<()> {
    let args = Args::parse();

    let non_interactive = args.non_interactive;

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

    let mut work_queue = WorkQueue::new(settings.queue_path)?;

    let max_retry_time = Duration::from_secs(settings.max_retry_time);

    let cancel_token = CancellationToken::new();
    let cloned_token = cancel_token.clone();

    //TODO: more graceful shutdown
    tokio::spawn(async move {
        let mut prev_client = None;
        let mut err;

        loop {
            (prev_client, err) = scrobble_task(
                &mut rx,
                &mut work_queue,
                prev_client,
                &settings.sk_path,
                max_retry_time,
                non_interactive,
            )
            .await;

            match err {
                MsgHandleError::ChannelClosed => info!("message channel closed"),
                MsgHandleError::LastFmFatal(_) => cloned_token.cancel(),
                MsgHandleError::LastFmReauth(_) => continue,
            }
            break;
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

    if cancel_token.is_cancelled() {
        return Err(anyhow!("unrecoverable error, shutting down"));
    }

    if let Some(Ok(info)) = current_song.as_ref().map(TryInto::try_into) {
        tx.send(Message::NowPlaying(Some(info))).await?;
    }

    loop {
        select! {
        _ = cancel_token.cancelled() => return Err(anyhow!("unrecoverable error, shutting down")),
        s = tokio::signal::ctrl_c() => match s {
            Ok(_) => {
                eprintln!();
                break;
            }
            // why would this ever happen?
            Err(e) => {
                eprintln!();
                error!("huh? {e}");
                break;
            }
        },
        n = state_changes.next() => match n {
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
            _ => {
                error!("lost connection to MPD");
                break;
            }
        }}
    }

    Ok(())
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
                    tx.send(Message::NowPlaying(Some(info))).await?;
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

            match new_song.as_ref().map(TryInto::try_into) {
                Some(Ok(info)) => tx.send(Message::NowPlaying(Some(info))).await?,
                Some(Err(e)) => warn!("couldn't update now playing: {e}"),
                None => tx.send(Message::NowPlaying(None)).await?,
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

async fn scrobble_task(
    rx: &mut mpsc::Receiver<Message>,
    work_queue: &mut WorkQueue,
    prev_client: Option<LastFmClient>,
    sk_path: &Path,
    max_retry_time: Duration,
    non_interactive: bool,
) -> (Option<LastFmClient>, MsgHandleError) {
    let client_future = LastFmClient::new(prev_client, sk_path, non_interactive);

    tokio::pin!(client_future);

    let mut current_song = None;

    let mut retry_time = Duration::from_secs(15);
    let mut client = match loop {
        select! {
            r = &mut client_future => break r.map_err(Into::into),
            Some(msg) = rx.recv() => {
                match msg {
                    Message::Scrobble(info, timestamp) => work_queue.add_scrobble(info, timestamp),
                    Message::TrackAction(action, info) => work_queue.add_action(action, info),
                    Message::NowPlaying(info_opt) => {
                        if let Some(info) = info_opt.as_ref() {
                            info!("new song: {} - {}", info.artist, info.title);
                        }
                        current_song = info_opt;
                    }
                };
            }
            else => return (None, MsgHandleError::ChannelClosed),
        }
    } {
        Ok(client) => client,
        Err(e) => {
            error!("{e}");
            return (None, e);
        }
    };

    if let Some(info) = current_song {
        if let Ok(()) = client.now_playing(&info).await {
            info!("updated now playing status successfully");
        }
    }

    if work_queue.has_work() {
        if let Err(e) = work_queue.do_work(&mut client).await {
            if !e.is_retryable() {
                return (Some(client), e.into());
            }
        }
    }

    loop {
        retry_time = min(max_retry_time, retry_time);

        retry_time = match handle_async_msg(rx, retry_time, work_queue, &mut client).await {
            Ok(t) => t,
            Err(e) => break (Some(client), e),
        }
    }
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
                    Message::Scrobble(info, timestamp) => work_queue.add_scrobble(info, timestamp),
                    Message::TrackAction(action, info) => work_queue.add_action(action, info),
                    Message::NowPlaying(info_opt) => {
                        if let Some(info) = info_opt.as_ref() {
                            info!("new song: {} - {}", info.artist, info.title);
                        }
                        work_queue.last_played = info_opt;

                        // good heuristic for preventing calling do_work twice in quick succession
                        //
                        // we don't really care about retrying when now playing has changed without
                        // any scrobbles
                        return Ok(retry_time);
                    }
                };
                match work_queue.do_work(client).await {
                    Ok(_) => Ok(Duration::from_secs(15)),
                    Err(e) => if e.is_retryable() {
                        Ok(retry_time)
                    } else {
                        Err(e.into())
                    }
                }
            },
            () = t => match work_queue.do_work(client).await {
                    Ok(_) => Ok(Duration::from_secs(15)),
                    Err(e) => if e.is_retryable() {
                        Ok(retry_time * 2)
                    } else {
                        Err(e.into())
                    }
                },
            else => Err(MsgHandleError::ChannelClosed),
        }
    } else if let Some(msg) = r.await {
        match msg {
            Message::Scrobble(info, timestamp) => {
                info!("scrobbling {} - {}", info.artist, info.title);
                if let Err(e) = client.scrobble_one(&info, &timestamp).await {
                    if e.is_retryable() {
                        warn!("scrobble failed: {e}");
                        work_queue.add_scrobble(info, timestamp);
                    } else {
                        error!("scrobble failed: {e}");
                        work_queue.add_scrobble(info, timestamp);
                        return Err(e.into());
                    }
                } else {
                    info!("scrobbled successfully");
                }
            }
            Message::TrackAction(action, info) => {
                info!("{}ing {} - {}", action, info.artist, info.title);
                if let Err(e) = client.do_track_action(action, &info).await {
                    if e.is_retryable() {
                        warn!("{action}e track failed: {e}");
                        work_queue.add_action(action, info);
                    } else {
                        error!("{action}e track failed: {e}");
                        work_queue.add_action(action, info);
                        return Err(e.into());
                    }
                } else {
                    info!("{action}ed successfully");
                }
            }
            Message::NowPlaying(Some(info)) => {
                info!("new song: {} - {}", info.artist, info.title);
                if let Err(e) = client.now_playing(&info).await {
                    work_queue.last_played = Some(info);
                    if e.is_retryable() {
                        warn!("updating now playing failed: {e}");
                    } else {
                        error!("updating now playing failed: {e}");
                        return Err(e.into());
                    }
                } else {
                    info!("updated now playing status successfully");
                }
            }
            Message::NowPlaying(None) => {
                work_queue.last_played = None;
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
