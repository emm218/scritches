use std::{io, time::Duration};

use md5::{Digest, Md5};
use once_cell::sync::Lazy;
use reqwest::Client as HttpClient;
use serde::de::DeserializeOwned;
use serde_derive::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
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
static MBID: Lazy<[String; 50]> = with_indices!("mbid");
static TIMESTAMP: Lazy<[String; 50]> = with_indices!("timestamp");

#[derive(Debug, Serialize, Deserialize)]
pub struct SongInfo {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ScrobbleInfo {
    pub timestamp: String,
    pub song: SongInfo,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BasicInfo {
    pub title: String,
    pub artist: String,
}

trait PushParams<'a, T> {
    fn push_params(&mut self, info: &'a T);
    fn push_params_idx(&mut self, info: &'a T, idx: usize);
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
        if let Some(mbid) = info.track_id.as_ref() {
            self.push(("mbid", mbid));
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
        if let Some(mbid) = info.track_id.as_ref() {
            self.push((&MBID[idx], mbid));
        }
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
pub enum Message {
    Scrobble(ScrobbleInfo),
    NowPlaying(SongInfo),
    Action(Action),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Action {
    LoveTrack(BasicInfo),
    UnloveTrack(BasicInfo),
}

impl Message {
    pub fn love_track(info: BasicInfo) -> Self {
        Message::Action(Action::LoveTrack(info))
    }

    pub fn unlove_track(info: BasicInfo) -> Self {
        Message::Action(Action::UnloveTrack(info))
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

        let client = HttpClient::new();

        let token = unauth_method_call::<Token>("auth.getToken", None, &client)
            .await?
            .token;

        println!("token: {token}");

        let url = format!("{}?api_key={}&token={}", AUTH_URL, API_KEY, token);

        open::that(url)?;

        let mut retry = interval(Duration::from_secs(10));

        let session = loop {
            retry.tick().await;
            match unauth_method_call::<Session>(
                "auth.getSession",
                Some(vec![("token", &token[..])]),
                &client,
            )
            .await
            {
                Ok(Session { session }) => break Ok(session),
                Err(Error::Api(ApiError { error: 14, .. })) => {
                    println!("not authorized, retrying...")
                }
                Err(e) => break Err(e),
            }
        }?;

        println!("authenticated user {}", session.name);

        let session_key = session.key;

        println!("sk: {session_key}");

        Ok(Self {
            session_key,
            client,
        })
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
    params: Option<Vec<(&str, &str)>>,
    client: &HttpClient,
) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    let params = match params {
        Some(mut p) => {
            p.push(("method", method));
            p.push(("api_key", API_KEY));
            p
        }
        None => vec![("method", method), ("api_key", API_KEY)],
    };

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
