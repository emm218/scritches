use std::iter::once;

use md5::{Digest, Md5};
use serde_derive::{Deserialize, Serialize};
use serde_urlencoded::ser::Error as SerializeError;

static API_KEY: &str = "936df272ba862808520323da81f3fc6e";
static API_SECRET: &str = "d401bc1f1a702af8e6bd8c50bce9b11d";
static API_URL: &str = "https://ws.audioscrobbler.com/2.0/";

#[derive(Debug, Serialize, Deserialize)]
pub struct ScrobbleInfo {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_id: Option<String>,
    pub start_time: String,
}

impl ScrobbleInfo {
    pub fn push_params<'a>(&'a self, out: &mut Vec<(String, &'a str)>) {
        out.push(("title".into(), &self.title));
        out.push(("artist".into(), &self.artist));
        out.push(("timestamp".into(), &self.start_time));
        if let Some(album) = self.album.as_ref() {
            out.push(("album".into(), album));
        }
        if let Some(album_artist) = self.album_artist.as_ref() {
            out.push(("albumArtist".into(), album_artist));
        }
        if let Some(mbid) = self.track_id.as_ref() {
            out.push(("mbid".into(), mbid));
        }
    }

    pub fn push_params_idx<'a>(&'a self, idx: usize, out: &mut Vec<(String, &'a str)>) {
        out.push((format!("title[{idx}]"), &self.title));
        out.push((format!("artist[{idx}]"), &self.artist));
        out.push((format!("timestamp[{idx}]"), &self.start_time));
        if let Some(album) = self.album.as_ref() {
            out.push((format!("album[{idx}]"), album));
        }
        if let Some(album_artist) = self.album_artist.as_ref() {
            out.push((format!("albumArtist[{idx}]"), album_artist));
        }
        if let Some(mbid) = self.track_id.as_ref() {
            out.push((format!("mbid[{idx}]"), mbid));
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
}

pub struct Client(bool);

impl Client {
    pub fn new() -> Self {
        Self(true)
    }

    pub async fn scrobble_one(&mut self, info: &ScrobbleInfo) -> Result<(), Error> {
        let mut params = vec![
            ("method".to_string(), "track.scrobble"),
            ("api_key".to_string(), API_KEY),
        ];

        info.push_params(&mut params);

        println!("{}", method_call(&params)?);
        Ok(())
    }

    pub async fn scrobble_many(&mut self, infos: &[ScrobbleInfo]) -> Result<(), Error> {
        let mut params = vec![
            ("method".to_string(), "track.scrobble"),
            ("api_key".to_string(), API_KEY),
        ];

        for (i, info) in infos.iter().enumerate() {
            info.push_params_idx(i, &mut params)
        }

        println!("{}", method_call(&params)?);
        Ok(())
    }
}

fn method_call(params: &[(String, &str)]) -> Result<String, Error> {
    let mut sorted_params = params.iter().collect::<Vec<_>>();
    sorted_params.sort_unstable();

    let mut hasher = Md5::new();
    for (k, v) in sorted_params {
        hasher.update(k);
        hasher.update(v);
    }
    hasher.update(API_SECRET);

    let signature = hex::encode(&hasher.finalize()[..]);

    Ok(serde_urlencoded::to_string(
        params
            .iter()
            .chain(once(&("api_sig".to_string(), &signature[..])))
            .collect::<Vec<_>>(),
    )?)
}
