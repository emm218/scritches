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
    #[error("blaaaa")]
    Bla,
}

pub struct Client(bool);

impl Client {
    pub fn new() -> Self {
        Self(true)
    }

    pub async fn scrobble_one(&mut self, info: &ScrobbleInfo) -> Result<(), Error> {
        let x = rand::random::<f32>();
        if !self.0 {
            if x > 2.0 {
                println!("{} - {}", info.artist, info.title);
                self.0 = true;
                Ok(())
            } else {
                Err(Error::Bla)
            }
        } else if x > 0.8 {
            self.0 = false;
            Err(Error::Bla)
        } else {
            println!("{} - {}", info.artist, info.title);
            Ok(())
        }
    }

    pub async fn scrobble_many(&mut self, infos: &[ScrobbleInfo]) -> Result<(), Error> {
        let x = rand::random::<f32>();
        if !self.0 {
            if x > 2.0 {
                for info in infos {
                    println!("{} - {}", info.artist, info.title);
                }
                self.0 = true;
                Ok(())
            } else {
                Err(Error::Bla)
            }
        } else if x > 0.8 {
            self.0 = false;
            Err(Error::Bla)
        } else {
            for info in infos {
                println!("{} - {}", info.artist, info.title);
            }
            Ok(())
        }
    }
}
