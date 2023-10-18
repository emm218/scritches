use std::{
    cmp::min,
    collections::VecDeque,
    fs::File,
    io::{self, Seek},
    path::Path,
};

use crate::last_fm::{Action, Client as LastFmClient, Error as LastFmError, ScrobbleInfo};

#[derive(Debug)]
pub struct WorkQueue {
    scrobble_queue: VecDeque<ScrobbleInfo>,
    action_queue: VecDeque<Action>,
    queue_file: File,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    BinCode(#[from] bincode::Error),
    #[error(transparent)]
    LastFm(#[from] LastFmError),
}

impl WorkQueue {
    pub fn new(path: &Path) -> io::Result<Self> {
        let f = File::open(path)?;
        let (scrobble_queue, action_queue) = bincode::deserialize_from(f).unwrap_or_else(|e| {
            eprintln!("unable to load queue file: {e}");
            (VecDeque::new(), VecDeque::new())
        });

        let queue_file = File::create(path)?;

        Ok(Self {
            scrobble_queue,
            action_queue,
            queue_file,
        })
    }

    pub fn write(&mut self) -> bincode::Result<()> {
        self.queue_file.set_len(0)?;
        self.queue_file.rewind()?;
        bincode::serialize_into(
            &self.queue_file,
            &(&self.scrobble_queue, &self.action_queue),
        )
    }

    pub fn has_work(&self) -> bool {
        !(self.scrobble_queue.is_empty() && self.action_queue.is_empty())
    }

    pub async fn do_work(&mut self, client: &mut LastFmClient) -> Result<(), Error> {
        println!("scrobbling from queue: {} songs", self.scrobble_queue.len());
        while !self.scrobble_queue.is_empty() {
            println!("batch");
            let range = ..min(4, self.scrobble_queue.len());
            let batch = &self.scrobble_queue.make_contiguous()[range];
            client.scrobble_many(batch).await?;
            self.scrobble_queue.drain(range);
        }
        self.write()?;
        Ok(())
    }

    pub fn add_scrobble(&mut self, info: ScrobbleInfo) -> bincode::Result<()> {
        self.scrobble_queue.push_back(info);
        self.write()
    }

    pub fn add_action(&mut self, action: Action) -> bincode::Result<()> {
        self.action_queue.push_back(action);
        self.write()
    }
}