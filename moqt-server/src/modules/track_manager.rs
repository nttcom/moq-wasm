use anyhow::Result;
use async_trait::async_trait;
use moqt_core::TrackManagerRepository;
use std::collections::{hash_map::Entry, HashMap};
use tokio::sync::{mpsc, oneshot};
use TrackCommand::*;

// Called as a separate thread
pub(crate) async fn track_manager(rx: &mut mpsc::Receiver<TrackCommand>) {
    tracing::info!("track_manager start");

    let mut tracks = HashMap::<String, usize>::new();

    while let Some(cmd) = rx.recv().await {
        tracing::info!("command received");
        match cmd {
            Set {
                track_namespace,
                session_id,
                resp,
            } => {
                // 既存の値があると更新されないでfalseが返る
                match tracks.entry(track_namespace) {
                    Entry::Vacant(track) => {
                        track.insert(session_id);
                        resp.send(true).unwrap();
                    }
                    Entry::Occupied(_) => resp.send(false).unwrap(),
                };
            }
            Delete {
                track_namespace,
                resp,
            } => {
                let removed_value = tracks.remove(&track_namespace);
                match removed_value {
                    Some(_) => resp.send(true).unwrap(),
                    None => resp.send(false).unwrap(),
                }
            }
            Has {
                track_namespace,
                resp,
            } => {
                let result = tracks.get(&track_namespace);
                match result {
                    Some(_) => resp.send(true).unwrap(),
                    None => resp.send(false).unwrap(),
                }
            }
            GetPublisherSessionId {
                track_namespace,
                resp,
            } => {
                let session_id = tracks.get(&track_namespace).copied();
                resp.send(session_id).unwrap();
            }
        }
    }

    tracing::info!("track_manager end");
}

#[derive(Debug)]
pub(crate) enum TrackCommand {
    Set {
        track_namespace: String,
        session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    Delete {
        track_namespace: String,
        resp: oneshot::Sender<bool>,
    },
    Has {
        track_namespace: String,
        resp: oneshot::Sender<bool>,
    },
    GetPublisherSessionId {
        track_namespace: String,
        resp: oneshot::Sender<Option<usize>>,
    },
}

// channel周りの処理を隠蔽するためのラッパー
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
    async fn set(&self, track_namespace: &str, session_id: usize) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::Set {
            track_namespace: track_namespace.to_string(),
            session_id,
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
    async fn delete(&self, track_namespace: &str) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::Delete {
            track_namespace: track_namespace.to_string(),
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
    async fn has(&self, track_namespace: &str) -> bool {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::Has {
            track_namespace: track_namespace.to_string(),
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();
        return result;
    }

    // track_namespaceからpublisherのsession_idを取得する
    async fn get_publisher_session_id_by_track_namespace(
        &self,
        track_namespace: &str,
    ) -> Option<usize> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<usize>>();
        let cmd = TrackCommand::GetPublisherSessionId {
            track_namespace: track_namespace.to_string(),
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let session_id = resp_rx.await.unwrap();

        return session_id;
    }
}
