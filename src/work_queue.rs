use std::{
    cmp::min,
    collections::VecDeque,
    fs::File,
    io::{self, Seek},
    path::Path,
};

use log::{error, info, trace, warn};

use crate::last_fm::{Action, BasicInfo, Client as LastFmClient, Error as LastFmError, SongInfo};

#[derive(Debug)]
pub struct WorkQueue {
    scrobble_queue: VecDeque<(SongInfo, String)>,
    action_queue: VecDeque<(Action, BasicInfo)>,
    queue_file: File,
    pub last_played: Option<SongInfo>,
}

impl WorkQueue {
    pub fn new(path: &Path) -> io::Result<Self> {
        let (scrobble_queue, action_queue) = match File::open(path) {
            Ok(f) => bincode::deserialize_from(f).unwrap_or_else(|e| {
                warn!("unable to read queue file: {e}");
                (VecDeque::new(), VecDeque::new())
            }),
            Err(_) => (VecDeque::new(), VecDeque::new()),
        };

        let queue_file = File::create(path)?;

        let mut res = Self {
            scrobble_queue,
            action_queue,
            queue_file,
            last_played: None,
        };

        res.write();
        Ok(res)
    }

    fn write(&mut self) {
        if let Err(e) = self.try_write() {
            error!("failed to save work queue: {e}");
        }
    }

    fn try_write(&mut self) -> bincode::Result<()> {
        self.queue_file.set_len(0)?;
        self.queue_file.rewind()?;
        bincode::serialize_into(
            &self.queue_file,
            &(&self.scrobble_queue, &self.action_queue),
        )
    }

    #[inline]
    pub fn has_work(&self) -> bool {
        !self.scrobble_queue.is_empty()
            || !self.action_queue.is_empty()
            || self.last_played.is_some()
    }

    pub async fn do_work(&mut self, client: &mut LastFmClient) -> Result<(), LastFmError> {
        let mut count = 0;
        while !self.scrobble_queue.is_empty() {
            let range = ..min(50, self.scrobble_queue.len());
            let batch = &self.scrobble_queue.make_contiguous()[range];
            if let Err(e) = client.scrobble_many(batch).await {
                self.write();
                if e.is_retryable() {
                    warn!("scrobbling queue failed: {e}");
                } else {
                    error!("scrobbling queue failed: {e}");
                }
                if count > 0 {
                    info!("succesfully scrobbled {count} songs from queue");
                }
                return Err(e);
            }
            count += range.end;
            self.scrobble_queue.drain(range);
        }
        info!("succesfully scrobbled {count} songs from queue");

        while let Some((action, info)) = self.action_queue.front() {
            if let Err(e) = client.do_track_action(*action, info).await {
                error!("{action}e track failed: {e}");
                self.write();
                return Err(e);
            }
        }
        self.write();

        if let Some(info) = self.last_played.as_ref() {
            client.now_playing(info).await?;
            self.last_played = None;
            info!("succesfully updated now playing status");
        }

        Ok(())
    }

    pub fn add_scrobble(&mut self, info: SongInfo, timestamp: String) {
        info!("added scrobble {} - {} to queue", info.artist, info.title);
        self.scrobble_queue.push_back((info, timestamp));
        self.write();
    }

    pub fn add_action(&mut self, action: Action, info: BasicInfo) {
        info!("added {action}e {} - {} to queue", info.artist, info.title);
        self.action_queue.push_back((action, info));
        self.write();
    }
}
