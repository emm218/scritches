use serde_derive::{Deserialize, Serialize};

use std::time::SystemTime;

#[derive(Debug, Serialize, Deserialize)]
pub struct ScrobbleInfo {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_id: Option<String>,
    pub start_time: SystemTime,
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
    LoveTrack { title: String, artist: String },
    UnloveTrack { title: String, artist: String },
}

impl Message {
    pub fn love_track(info: BasicInfo) -> Self {
        Message::Action(Action::LoveTrack {
            title: info.title,
            artist: info.artist,
        })
    }

    pub fn unlove_track(info: BasicInfo) -> Self {
        Message::Action(Action::UnloveTrack {
            title: info.title,
            artist: info.artist,
        })
    }
}
