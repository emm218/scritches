use std::{io, time::Duration};

use md5::{Digest, Md5};
use mpd_client::{
    responses::{Song, SongInQueue},
    tag::Tag,
};
use once_cell::sync::Lazy;
use reqwest::Client as HttpClient;
use serde::de::DeserializeOwned;
use serde_derive::{Deserialize, Serialize};
use tokio::time::interval;

static API_KEY: &str = "936df272ba862808520323da81f3fc6e";
static API_SECRET: &str = "d401bc1f1a702af8e6bd8c50bce9b11d";
static API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
static AUTH_URL: &str = "https://www.last.fm/api/auth/";

macro_rules! with_indices {
    ($l:literal) => {
        Lazy::new(|| array_init::array_init(|i| format!(concat!($l, "[{}]"), i)))
    };
}

// this is mildly evil but prevents extra memory allocations, yippee!!!
static TRACK: Lazy<[String; 50]> = with_indices!("track");
static ARTIST: Lazy<[String; 50]> = with_indices!("artist");
static ALBUM: Lazy<[String; 50]> = with_indices!("album");
static ALBUMARTIST: Lazy<[String; 50]> = with_indices!("albumArtist");
static TIMESTAMP: Lazy<[String; 50]> = with_indices!("timestamp");
static DURATION: Lazy<[String; 50]> = with_indices!("duration");

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

//TODO: I think there's a way to get rid of this type which would make SEVERAL things easier
#[derive(Debug, Serialize, Deserialize)]
pub struct ScrobbleInfo {
    pub timestamp: String,
    pub song: SongInfo,
}

impl ScrobbleInfo {
    pub fn try_from<T>(s: T, start_time: Duration) -> Result<Self, SongError>
    where
        T: TryInto<SongInfo, Error = SongError>,
    {
        let song = s.try_into()?;

        Ok(Self {
            song,
            timestamp: start_time.as_secs().to_string(),
        })
    }
}

impl<'a> PushParams<'a, ScrobbleInfo> for Vec<(&str, &'a str)> {
    fn push_params(&mut self, info: &'a ScrobbleInfo) {
        self.push(("timestamp", &info.timestamp));
        self.push_params(&info.song);
    }

    fn push_params_idx(&mut self, info: &'a ScrobbleInfo, idx: usize) {
        self.push((&TIMESTAMP[idx], &info.timestamp));
        self.push_params_idx(&info.song, idx);
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

#[derive(Debug, Serialize, Deserialize)]
pub enum Message {
    Scrobble(ScrobbleInfo),
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

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("too many scrobbles in batch. maximum is 50 got {0}")]
    TooManyScrobbles(usize),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Api(#[from] ApiError),

    #[error("error deserializing response: {0}")]
    Ser(#[from] serde_json::Error),

    #[error("couldn't open browser for authentication: {0}")]
    Open(#[from] io::Error),
}

#[derive(Debug, Deserialize, thiserror::Error)]
#[error("api error {error}: {message}")]
pub struct ApiError {
    error: u32,
    message: String,
}

pub struct Client {
    session_key: String,
    client: HttpClient,
}

impl Client {
    pub async fn new() -> Result<Self, Error> {
        let client = HttpClient::new();

        let session_key = Self::authenticate(&client).await?;

        Ok(Self {
            session_key,
            client,
        })
    }

    async fn authenticate(client: &HttpClient) -> Result<String, Error> {
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

        println!("token: {token}");

        let url = format!("{}?api_key={}&token={}", AUTH_URL, API_KEY, token);

        println!(
            "authorization page should open automatically, if not then go to {url} to authorize"
        );

        let _ = open::that(url);

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
                Err(Error::Api(ApiError { error: 14, .. })) => {
                    println!("not authorized, retrying...");
                }
                Err(e) => break Err(e),
            }
        }?;

        println!("authorized for user {}", session.name);

        Ok(session.key)
    }

    async fn method_call<T>(
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
    }

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

        Ok(())
    }

    pub async fn scrobble_one(&mut self, info: &ScrobbleInfo) -> Result<(), Error> {
        let mut params = Vec::new();

        params.push_params(info);

        self.void_method("track.scrobble", Some(params)).await
    }

    pub async fn scrobble_many(&mut self, infos: &[ScrobbleInfo]) -> Result<(), Error> {
        if infos.len() > 50 {
            return Err(Error::TooManyScrobbles(infos.len()));
        }
        let mut params = Vec::new();

        infos
            .iter()
            .enumerate()
            .for_each(|(i, info)| params.push_params_idx(info, i));

        self.void_method("track.scrobble", Some(params)).await
    }

    pub async fn now_playing(&mut self, info: &SongInfo) -> Result<(), Error> {
        let mut params = Vec::new();

        params.push_params(info);

        self.void_method("track.updateNowPlaying", Some(params))
            .await
    }

    pub async fn do_track_action(&mut self, action: Action, info: &BasicInfo) -> Result<(), Error> {
        let mut params = Vec::new();

        params.push_params(info);

        match action {
            Action::Love => self.void_method("track.love", Some(params)).await,
            Action::Unlove => self.void_method("track.unlove", Some(params)).await,
        }
    }
}

struct SignedParams<'a, 'b> {
    params: Vec<(&'a str, &'b str)>,
    signature: String,
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
}
