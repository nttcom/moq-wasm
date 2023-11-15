use std::collections::HashSet;

use anyhow::Result;
use async_trait::async_trait;
use moqt_core::TrackManagerRepository;
use tokio::sync::{mpsc, oneshot};

pub(crate) async fn track_manager(rx: &mut mpsc::Receiver<TrackCommand>) {
    tracing::info!("track_manager start");

    let mut tracks = HashSet::<String>::new();

    use TrackCommand::*;
    while let Some(cmd) = rx.recv().await {
        tracing::info!("command received");
        match cmd {
            Set { track_name, resp } => {
                // 既存の値があると更新されないでfalseが返る
                let result = tracks.insert(track_name);
                resp.send(result).unwrap();
            }
            Delete { track_name, resp } => {
                let result = tracks.remove(&track_name);
                resp.send(result).unwrap();
            }
            Has { track_name, resp } => {
                let result = tracks.contains(&track_name);
                resp.send(result).unwrap();
            }
        }
    }

    tracing::info!("track_manager end");
}

#[derive(Debug)]
pub(crate) enum TrackCommand {
    Set {
        track_name: String,
        resp: oneshot::Sender<bool>,
    },
    Delete {
        track_name: String,
        resp: oneshot::Sender<bool>,
    },
    Has {
        track_name: String,
        resp: oneshot::Sender<bool>,
    },
}

pub(crate) struct TrackManager {
    tx: mpsc::Sender<TrackCommand>,
}

impl TrackManager {
    pub fn new(tx: mpsc::Sender<TrackCommand>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl TrackManagerRepository for TrackManager {
    async fn set(&self, track_name: &str) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::Set {
            track_name: track_name.to_string(),
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        if result {
            Ok(())
        } else {
            Err(anyhow::anyhow!("already exist"))
        }
    }
    async fn delete(&self, track_name: &str) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::Delete {
            track_name: track_name.to_string(),
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        if result {
            Ok(())
        } else {
            Err(anyhow::anyhow!("not found"))
        }
    }
    async fn has(&self, track_name: &str) -> bool {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::Has {
            track_name: track_name.to_string(),
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        resp_rx.await.unwrap()
    }
}
