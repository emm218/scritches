use std::{fmt, fs, path::Path, sync::LazyLock, time::Duration};

use log::{debug, error, info, trace, warn};
use md5::{Digest, Md5};
use mpd_client::responses::{Song, SongInQueue};
use reqwest::{Client as HttpClient, RequestBuilder};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::time::interval;

static API_KEY: &str = "936df272ba862808520323da81f3fc6e";
static API_SECRET: &str = "d401bc1f1a702af8e6bd8c50bce9b11d";
static API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
static AUTH_URL: &str = "https://www.last.fm/api/auth/";

macro_rules! with_indices {
    ($l:literal) => {
        std::sync::LazyLock::new(|| array_init::array_init(|i| format!(concat!($l, "[{}]"), i)))
    };
}

// this is mildly evil but prevents extra memory allocations, yippee!!!
static TRACK: LazyLock<[String; 50]> = with_indices!("track");
static ARTIST: LazyLock<[String; 50]> = with_indices!("artist");
static ALBUM: LazyLock<[String; 50]> = with_indices!("album");
static ALBUMARTIST: LazyLock<[String; 50]> = with_indices!("albumArtist");
static TIMESTAMP: LazyLock<[String; 50]> = with_indices!("timestamp");
static DURATION: LazyLock<[String; 50]> = with_indices!("duration");

trait PushParams<'a, T> {
    fn push_params(&mut self, info: &'a T);
    fn push_params_idx(&mut self, info: &'a T, idx: usize);
}

#[derive(Debug, thiserror::Error)]
pub enum SongError {
    #[error("title is missing")]
    NoTitle,
    #[error("artist is missing")]
    NoArtist,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SongInfo {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub duration: Option<String>,
}

impl TryFrom<&Song> for SongInfo {
    type Error = SongError;

    fn try_from(song: &Song) -> Result<Self, Self::Error> {
        let title = song.title().ok_or(Self::Error::NoTitle)?.to_string();
        let artist = (!song.artists().is_empty())
            .then(|| song.artists().join(", "))
            .ok_or(Self::Error::NoArtist)?;
        let album = song.album().map(ToString::to_string);
        let album_artist = (!song.album_artists().is_empty())
            .then(|| song.album_artists().join(", "))
            .filter(|a| a.ne(&artist));
        let duration = song.duration.map(|d| d.as_secs().to_string());
        Ok(Self {
            title,
            artist,
            album,
            album_artist,
            duration,
        })
    }
}

impl TryFrom<&SongInQueue> for SongInfo {
    type Error = SongError;

    fn try_from(song: &SongInQueue) -> Result<Self, Self::Error> {
        Self::try_from(&song.song)
    }
}

impl<'a> PushParams<'a, SongInfo> for Vec<(&str, &'a str)> {
    fn push_params(&mut self, info: &'a SongInfo) {
        self.push(("track", &info.title));
        self.push(("artist", &info.artist));
        if let Some(album) = info.album.as_ref() {
            self.push(("album", album));
        }
        if let Some(album_artist) = info.album_artist.as_ref() {
            self.push(("albumArtist", album_artist));
        }
        if let Some(duration) = info.duration.as_ref() {
            self.push(("duration", duration));
        }
    }

    fn push_params_idx(&mut self, info: &'a SongInfo, idx: usize) {
        self.push((&TRACK[idx], &info.title));
        self.push((&ARTIST[idx], &info.artist));
        if let Some(album) = info.album.as_ref() {
            self.push((&ALBUM[idx], album));
        }
        if let Some(album_artist) = info.album_artist.as_ref() {
            self.push((&ALBUMARTIST[idx], album_artist));
        }
        if let Some(duration) = info.duration.as_ref() {
            self.push((&DURATION[idx], duration));
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BasicInfo {
    pub title: String,
    pub artist: String,
}

impl TryFrom<&Song> for BasicInfo {
    type Error = SongError;

    fn try_from(song: &Song) -> Result<Self, Self::Error> {
        let title = song.title().ok_or(Self::Error::NoTitle)?.to_string();
        let artist = (!song.artists().is_empty())
            .then(|| song.artists().join(", "))
            .ok_or(Self::Error::NoArtist)?;
        Ok(Self { title, artist })
    }
}

impl TryFrom<&SongInQueue> for BasicInfo {
    type Error = SongError;

    fn try_from(song: &SongInQueue) -> Result<Self, Self::Error> {
        Self::try_from(&song.song)
    }
}

impl<'a> PushParams<'a, BasicInfo> for Vec<(&str, &'a str)> {
    fn push_params(&mut self, info: &'a BasicInfo) {
        self.push(("track", &info.title));
        self.push(("artist", &info.artist));
    }

    fn push_params_idx(&mut self, info: &'a BasicInfo, idx: usize) {
        self.push((&TRACK[idx], &info.title));
        self.push((&ARTIST[idx], &info.artist));
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum Action {
    Love,
    Unlove,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Love => write!(f, "lov"),
            Self::Unlove => write!(f, "unlov"),
        }
    }
}

#[derive(Debug, Deserialize, thiserror::Error)]
#[error("{message} (error {error})")]
pub struct ApiError {
    error: u32,
    message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("too many scrobbles in batch. maximum is 50 got {0}")]
    TooManyScrobbles(usize),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("{1} (error {0})")]
    ApiRetry(u32, String),

    #[error("{1} (error {0})")]
    ApiReauth(u32, String),

    #[error("{1} (error {0})")]
    ApiFatal(u32, String),

    #[error("error deserializing response: {0}")]
    Ser(#[from] serde_json::Error),

    #[error("need interaction for authentication")]
    NonInteractive,
}

impl From<ApiError> for Error {
    fn from(e: ApiError) -> Self {
        match e.error {
            11 | 16 | 29 => Self::ApiRetry(e.error, e.message),
            14 | 15 | 9 => Self::ApiReauth(e.error, e.message),
            _ => Self::ApiFatal(e.error, e.message),
        }
    }
}

impl Error {
    #[inline]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Http(_) | Self::ApiRetry(_, _))
    }
}

struct SignedParams<'a, 'b> {
    params: Vec<(&'a str, &'b str)>,
    signature: String,
}

impl<'a, 'b> SignedParams<'a, 'b> {
    fn into_request(self, client: &HttpClient) -> RequestBuilder {
        let mut params = self.params;
        let signature = self.signature;

        params.push(("api_sig", &signature));
        params.push(("format", "json"));

        client.post(API_URL).form(&params)
    }
}

fn sign<'a, 'b>(mut params: Vec<(&'a str, &'b str)>) -> SignedParams<'a, 'b> {
    params.sort_unstable();

    let mut hasher = Md5::new();
    for (k, v) in &params[..] {
        hasher.update(k);
        hasher.update(v);
    }
    hasher.update(API_SECRET);

    let signature = hex::encode(&hasher.finalize()[..]);

    SignedParams { params, signature }
}

pub async fn unauth_method_call<T>(
    method: &str,
    args: Option<Vec<(&str, &str)>>,
    client: &HttpClient,
) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    let mut params = vec![("method", method), ("api_key", API_KEY)];

    if let Some(mut a) = args {
        params.append(&mut a);
    }

    let response = sign(params)
        .into_request(client)
        .send()
        .await?
        .text()
        .await?;

    if let Ok(e) = serde_json::from_str::<ApiError>(&response) {
        return Err(e.into());
    }

    Ok(serde_json::from_str(&response)?)
}

pub struct Client {
    session_key: String,
    client: HttpClient,
}

impl Client {
    // awful awful hack to deal with opaque future types, constructor can take a previous client to
    // reauth it instead of actually creating a new one
    pub async fn new(
        prev_client: Option<Self>,
        sk_path: &Path,
        non_interactive: bool,
    ) -> Result<Self, Error> {
        if let Some(prev_client) = prev_client {
            if non_interactive {
                return Err(Error::NonInteractive);
            }
            return prev_client.re_auth(sk_path).await;
        }

        let client = HttpClient::new();

        let session_key = match Self::retrieve_sk(sk_path) {
            Some(sk) => sk,
            None => {
                if non_interactive {
                    Err(Error::NonInteractive)
                } else {
                    Self::authenticate(&client, sk_path).await
                }?
            }
        };

        Ok(Self {
            session_key,
            client,
        })
    }

    // TODO: want this to be able to persist session key in dbus secrets service if available
    // instead of just in a file
    fn retrieve_sk(path: &Path) -> Option<String> {
        match std::fs::read_to_string(path) {
            Err(e) => {
                warn!("couldn't read session key from `{}`: {e}", path.display());
                None
            }
            Ok(sk) => Some(sk),
        }
    }

    async fn re_auth(mut self, sk_path: &Path) -> Result<Self, Error> {
        let session_key = Self::authenticate(&self.client, sk_path).await?;

        self.session_key = session_key;

        Ok(self)
    }

    async fn authenticate(client: &HttpClient, path: &Path) -> Result<String, Error> {
        #[derive(Debug, Deserialize)]
        struct Token {
            token: String,
        }

        #[derive(Debug, Deserialize)]
        struct SessionInner {
            name: String,
            key: String,
        }

        #[derive(Debug, Deserialize)]
        struct Session {
            session: SessionInner,
        }

        let token = unauth_method_call::<Token>("auth.getToken", None, client)
            .await?
            .token;

        debug!("token: {token}");

        let url = format!("{}?api_key={}&token={}", AUTH_URL, API_KEY, token);

        println!("authorization page should open automatically");

        if let Err(e) = open::that(&url) {
            warn!("couldn't open browser: {e}");
            println!("go to {url} to authorize");
        }

        let mut retry = interval(Duration::from_secs(5));
        retry.tick().await;

        let session = loop {
            retry.tick().await;
            match unauth_method_call::<Session>(
                "auth.getSession",
                Some(vec![("token", &token[..])]),
                client,
            )
            .await
            {
                Ok(Session { session }) => break Ok(session),
                Err(Error::ApiReauth(14, _)) => {
                    trace!("not authorized, retrying...");
                }
                Err(Error::ApiReauth(15, msg)) => {
                    warn!("token expired before authorization");
                    break Err(Error::ApiReauth(15, msg));
                }
                Err(e) => break Err(e),
            }
        }?;

        info!(
            "session key {} authorized for user {}",
            session.key, session.name
        );

        if let Err(e) = fs::write(path, &session.key) {
            warn!("failed to persist session key: {e}");
        }

        Ok(session.key)
    }

    /* async fn method_call<T>(
        &self,
        method: &str,
        args: Option<Vec<(&str, &str)>>,
    ) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let mut params = vec![
            ("method", method),
            ("api_key", API_KEY),
            ("sk", &self.session_key),
        ];

        if let Some(mut a) = args {
            params.append(&mut a);
        }

        let client = &self.client;

        let signed = sign(params);
        let request = client.post(API_URL).form(
            &signed
                .params
                .iter()
                .chain(vec![("api_sig", &signed.signature[..]), ("format", "json")].iter())
                .collect::<Vec<_>>(),
        );
        let response = request.send().await?.text().await?;

        println!("{response}");

        if let Ok(e) = serde_json::from_str::<ApiError>(&response) {
            return Err(e.into());
        }

        Ok(serde_json::from_str(&response)?)
    } */

    async fn void_method(
        &self,
        method: &str,
        args: Option<Vec<(&str, &str)>>,
    ) -> Result<(), Error> {
        let mut params = vec![
            ("method", method),
            ("api_key", API_KEY),
            ("sk", &self.session_key),
        ];

        if let Some(mut a) = args {
            params.append(&mut a);
        }

        let client = &self.client;

        let response = sign(params)
            .into_request(client)
            .send()
            .await?
            .text()
            .await?;

        if let Ok(e) = serde_json::from_str::<ApiError>(&response) {
            return Err(e.into());
        }

        Ok(())
    }

    pub async fn scrobble_one(&mut self, info: &SongInfo, timestamp: &str) -> Result<(), Error> {
        trace!("scrobble:{info:#?} timestamp: {timestamp}");
        let mut params = Vec::new();

        params.push_params(info);
        params.push(("timestamp", timestamp));

        self.void_method("track.scrobble", Some(params)).await
    }

    pub async fn scrobble_many(&mut self, infos: &[(SongInfo, String)]) -> Result<(), Error> {
        if infos.len() > 50 {
            return Err(Error::TooManyScrobbles(infos.len()));
        }
        let mut params = Vec::new();

        for (i, (info, timestamp)) in infos.iter().enumerate() {
            params.push_params_idx(info, i);
            params.push((&TIMESTAMP[i], timestamp));
        }

        self.void_method("track.scrobble", Some(params)).await
    }

    pub async fn now_playing(&mut self, info: &SongInfo) -> Result<(), Error> {
        trace!("now_playing: {info:#?}");
        let mut params = Vec::new();

        params.push_params(info);

        self.void_method("track.updateNowPlaying", Some(params))
            .await
    }

    pub async fn do_track_action(&mut self, action: Action, info: &BasicInfo) -> Result<(), Error> {
        trace!("do_track_action: {action:?} {info:#?}");
        let mut params = Vec::new();

        params.push_params(info);

        match action {
            Action::Love => self.void_method("track.love", Some(params)).await,
            Action::Unlove => self.void_method("track.unlove", Some(params)).await,
        }
    }
}
