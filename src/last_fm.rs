use md5::{Digest, Md5};
use once_cell::sync::Lazy;
use reqwest::Client as HttpClient;
use serde_derive::{Deserialize, Serialize};
use serde_urlencoded::ser::Error as SerializeError;

static API_KEY: &str = "936df272ba862808520323da81f3fc6e";
static API_SECRET: &str = "d401bc1f1a702af8e6bd8c50bce9b11d";
static API_URL: &str = "https://ws.audioscrobbler.com/2.0/";

macro_rules! with_indices {
    ($l:literal) => {
        Lazy::new(|| array_init::array_init(|i| format!(concat!($l, "[{}]"), i)))
    };
}

// this is mildly evil but prevents extra memory allocations, yippee!!!
static TITLE: Lazy<[String; 50]> = with_indices!("title");
static ARTIST: Lazy<[String; 50]> = with_indices!("artist");
static ALBUM: Lazy<[String; 50]> = with_indices!("album");
static ALBUMARTIST: Lazy<[String; 50]> = with_indices!("albumArtist");
static MBID: Lazy<[String; 50]> = with_indices!("mbid");
static TIMESTAMP: Lazy<[String; 50]> = with_indices!("timestamp");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScrobbleInfo {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_id: Option<String>,
    pub start_time: String,
}

impl ScrobbleInfo {
    pub fn push_params(&self, out: &mut Vec<(&str, String)>) {
        let clone = self.clone();

        out.push(("title", clone.title));
        out.push(("artist", clone.artist));
        out.push(("timestamp", clone.start_time));
        if let Some(album) = clone.album {
            out.push(("album", album));
        }
        if let Some(album_artist) = clone.album_artist {
            out.push(("albumArtist", album_artist));
        }
        if let Some(mbid) = clone.track_id {
            out.push(("mbid", mbid));
        }
    }

    pub fn push_params_idx(&self, idx: usize, out: &mut Vec<(&str, String)>) {
        let clone = self.clone();

        out.push((&TITLE[idx], clone.title));
        out.push((&ARTIST[idx], clone.artist));
        out.push((&TIMESTAMP[idx], clone.start_time));
        if let Some(album) = clone.album {
            out.push((&ALBUM[idx], album));
        }
        if let Some(album_artist) = clone.album_artist {
            out.push((&ALBUMARTIST[idx], album_artist));
        }
        if let Some(mbid) = clone.track_id {
            out.push((&MBID[idx], mbid));
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BasicInfo {
    pub title: String,
    pub artist: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Message {
    Scrobble(ScrobbleInfo),
    NowPlaying(BasicInfo),
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
    #[error(transparent)]
    Serialize(#[from] SerializeError),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("too many scrobbles in batch. maximum is 50 got {0}")]
    TooManyScrobbles(usize),
}

pub struct Client {
    session_key: Option<String>,
    client: HttpClient,
}

impl Client {
    pub async fn new() -> Result<Self, Error> {
        let client = HttpClient::new();

        let params = sign(vec![
            ("method", "auth.getToken".into()),
            ("api_key", API_KEY.into()),
        ]);

        let response = client.post(API_URL).form(&params).send().await?;
        let content = response.text().await?;

        println!("{content}");

        todo!();

        Ok(Self {
            session_key: None,
            client,
        })
    }

    pub async fn scrobble_one(&mut self, info: &ScrobbleInfo) -> Result<(), Error> {
        let mut params = vec![
            ("method", "track.scrobble".into()),
            ("api_key", API_KEY.into()),
        ];

        info.push_params(&mut params);

        Ok(())
    }

    pub async fn scrobble_many(&mut self, infos: &[ScrobbleInfo]) -> Result<(), Error> {
        if infos.len() > 50 {
            return Err(Error::TooManyScrobbles(infos.len()));
        }
        let mut params = vec![
            ("method", "track.scrobble".into()),
            ("api_key", API_KEY.into()),
        ];

        for (i, info) in infos.iter().enumerate() {
            info.push_params_idx(i, &mut params);
        }

        Ok(())
    }
}

fn sign(mut params: Vec<(&str, String)>) -> Vec<(&str, String)> {
    params.sort_unstable();

    let mut hasher = Md5::new();
    for (k, v) in &params[..] {
        hasher.update(k);
        hasher.update(v);
    }
    hasher.update(API_SECRET);

    let signature = hex::encode(&hasher.finalize()[..]);

    params.push(("api_sig", signature));
    params
}
