use anyhow::Result;
use async_trait::async_trait;
use moqt_core::TrackManagerRepository;
use tokio::sync::{mpsc, oneshot};
use TrackCommand::*;

#[derive(Clone)]
struct Publisher {
    track_namespace: String,
    publisher_session_id: usize,
    tracks: Vec<Track>,
}

#[derive(Clone)]
struct Track {
    track_id: usize,
    track_name: String,
    subscribers: Vec<Subscriber>,
}

#[derive(Clone)]
struct Subscriber {
    subscriber_session_id: usize,
}

fn is_exist_track_namespace(publishers: Vec<Publisher>, track_namespace: String) -> bool {
    publishers
        .iter()
        .any(|p| p.track_namespace == track_namespace)
}

fn is_exist_track_name(tracks: Vec<Track>, track_names: String) -> bool {
    tracks.iter().any(|t| t.track_name == track_names)
}

fn is_exist_subscriber(subscribers: Vec<Subscriber>, subscriber_session_id: usize) -> bool {
    subscribers
        .iter()
        .any(|s| s.subscriber_session_id == subscriber_session_id)
}

// Called as a separate thread
pub(crate) async fn track_manager(rx: &mut mpsc::Receiver<TrackCommand>) {
    tracing::info!("track_manager start");

    let mut publishers: Vec<Publisher> = Vec::new();

    while let Some(cmd) = rx.recv().await {
        tracing::info!("command received");
        match cmd {
            SetPublisher {
                track_namespace,
                publisher_session_id,
                resp,
            } => {
                let publisher = Publisher {
                    track_namespace: track_namespace.clone(),
                    publisher_session_id,
                    tracks: Vec::new(),
                };

                if !is_exist_track_namespace(publishers.clone(), publisher.track_namespace.clone())
                {
                    // track_namespaceが存在しない場合は追加する
                    publishers.push(publisher);

                    resp.send(true).unwrap();
                } else {
                    resp.send(false).unwrap();
                }
            }
            DeletePublisher {
                track_namespace,
                resp,
            } => {
                if let Some(pub_pos) = publishers
                    .iter()
                    .position(|p| p.track_namespace == track_namespace)
                {
                    publishers.remove(pub_pos);

                    resp.send(true).unwrap();
                } else {
                    resp.send(false).unwrap();
                }
            }
            HasNamespace {
                track_namespace,
                resp,
            } => {
                let result = is_exist_track_namespace(publishers.clone(), track_namespace);

                resp.send(result).unwrap();
            }
            GetPublisherSessionId {
                track_namespace,
                resp,
            } => {
                let result = publishers
                    .iter()
                    .position(|p| p.track_namespace == track_namespace)
                    .map(|pub_pos| publishers[pub_pos].publisher_session_id);

                resp.send(result).unwrap();
            }
            SetSubscliber {
                track_namespace,
                subscriber_session_id,
                track_id,
                track_name,
                resp,
            } => {
                // track_namespaceが存在するか確認する
                if let Some(pub_pos) = publishers
                    .iter()
                    .position(|p| p.track_namespace == track_namespace)
                {
                    // track_namespaceが存在する場合はtrack_nameが存在するか確認する
                    if !is_exist_track_name(publishers[pub_pos].tracks.clone(), track_name.clone())
                    {
                        // track_nameが存在しない場合は追加する
                        let track = Track {
                            track_id,
                            track_name,
                            subscribers: vec![Subscriber {
                                subscriber_session_id,
                            }],
                        };
                        publishers[pub_pos].tracks.push(track);

                        resp.send(true).unwrap();
                    } else {
                        // track_nameが存在する場合はsubscriberが存在するか確認する
                        if let Some(track_pos) = publishers[pub_pos]
                            .tracks
                            .iter()
                            .position(|t| t.track_name == track_name)
                        {
                            if !is_exist_subscriber(
                                publishers[pub_pos].tracks[track_pos].subscribers.clone(),
                                subscriber_session_id,
                            ) {
                                // subscriberが存在しない場合は追加する
                                let subscrber = Subscriber {
                                    subscriber_session_id,
                                };
                                publishers[pub_pos].tracks[track_pos]
                                    .subscribers
                                    .push(subscrber);

                                resp.send(true).unwrap();
                            } else {
                                // subscriberが存在する場合はfalseを返す
                                resp.send(false).unwrap();
                            }
                        }
                    }
                } else {
                    // track_namespaceが存在しない場合はfalseを返す
                    resp.send(false).unwrap();
                }
            }
            DeleteSubscliber {
                track_namespace,
                track_name,
                subscriber_session_id,
                resp,
            } => {
                // track_namespaceとtrack_nameに当てはまるsubscriberを削除する
                if let Some(pub_pos) = publishers
                    .iter()
                    .position(|p| p.track_namespace == track_namespace)
                {
                    if let Some(track_pos) = publishers[pub_pos]
                        .tracks
                        .iter()
                        .position(|t| t.track_name == track_name)
                    {
                        if let Some(sub_pos) = publishers[pub_pos].tracks[track_pos]
                            .subscribers
                            .iter()
                            .position(|s| s.subscriber_session_id == subscriber_session_id)
                        {
                            publishers[pub_pos].tracks[track_pos]
                                .subscribers
                                .remove(sub_pos);

                            // subscriberが他に存在しない場合はtrackも削除する
                            if publishers[pub_pos].tracks[track_pos].subscribers.is_empty() {
                                publishers[pub_pos].tracks.remove(track_pos);
                            }

                            resp.send(true).unwrap();
                        } else {
                            // subscriberが存在しない場合はfalseを返す
                            resp.send(false).unwrap();
                        }
                    } else {
                        // track_nameが存在しない場合はfalseを返す
                        resp.send(false).unwrap();
                    }
                } else {
                    // track_namespaceが存在しない場合はfalseを返す
                    resp.send(false).unwrap();
                }
            }
            GetSubscliberSessionId { track_id, resp } => {
                let result = publishers.iter().find_map(|p| {
                    p.tracks.iter().find_map(|t| {
                        if t.track_id == track_id {
                            Some(t.subscribers[0].subscriber_session_id)
                        } else {
                            None
                        }
                    })
                });

                resp.send(result).unwrap();
            }
        }
    }

    tracing::info!("track_manager end");
}

#[derive(Debug)]
pub(crate) enum TrackCommand {
    SetPublisher {
        track_namespace: String,
        publisher_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    DeletePublisher {
        track_namespace: String,
        resp: oneshot::Sender<bool>,
    },
    HasNamespace {
        track_namespace: String,
        resp: oneshot::Sender<bool>,
    },
    GetPublisherSessionId {
        track_namespace: String,
        resp: oneshot::Sender<Option<usize>>,
    },
    SetSubscliber {
        track_namespace: String,
        subscriber_session_id: usize,
        track_id: usize,
        track_name: String,
        resp: oneshot::Sender<bool>,
    },
    DeleteSubscliber {
        track_namespace: String,
        track_name: String,
        subscriber_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    GetSubscliberSessionId {
        track_id: usize,
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
    async fn set_publisher(
        &self,
        track_namespace: &str,
        publisher_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetPublisher {
            track_namespace: track_namespace.to_string(),
            publisher_session_id,
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

    async fn delete_publisher(&self, track_namespace: &str) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::DeletePublisher {
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

    async fn has_namespace(&self, track_namespace: &str) -> bool {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::HasNamespace {
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

    async fn set_subscriber(
        &self,
        track_namespace: &str,
        subscriber_session_id: usize,
        track_id: usize,
        track_name: &str,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetSubscliber {
            track_namespace: track_namespace.to_string(),
            subscriber_session_id,
            track_id,
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

    async fn delete_subscriber(
        &self,
        track_namespace: &str,
        track_name: &str,
        subscriber_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::DeleteSubscliber {
            track_namespace: track_namespace.to_string(),
            track_name: track_name.to_string(),
            subscriber_session_id,
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

    // track_idからsubscriberのsession_idを取得する
    async fn get_subscriber_session_id_by_track_id(&self, track_id: usize) -> Option<usize> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<usize>>();

        let cmd = TrackCommand::GetSubscliberSessionId {
            track_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let session_id = resp_rx.await.unwrap();

        return session_id;
    }
}

#[cfg(test)]
mod success {
    use crate::modules::track_manager::{track_manager, TrackManager, TrackManagerRepository};
    use crate::TrackCommand;
    use tokio::sync::mpsc;
    #[tokio::test]
    async fn set_publisher() {
        let track_namespace = "test";
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let result = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_publisher() {
        let track_namespace = "test";
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        let result = track_manager.delete_publisher(track_namespace).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn has_namespace() {
        let track_namespace = "test";
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        let result = track_manager.has_namespace(track_namespace).await;

        assert!(result);
    }

    #[tokio::test]
    async fn get_publisher_session_id_by_track_namespace() {
        let track_namespace = "test";
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        let result = track_manager
            .get_publisher_session_id_by_track_namespace(track_namespace)
            .await;

        assert_eq!(result, Some(publisher_session_id));
    }

    #[tokio::test]
    async fn set_subscriber_track_not_existed() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_id = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        // Register a new subscriber with a new track
        let result = track_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_id, track_name)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_subscriber_track_already_existed() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id_1 = 1;
        let subscriber_session_id_2 = 2;
        let track_id = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_manager
            .set_subscriber(
                track_namespace,
                subscriber_session_id_1,
                track_id,
                track_name,
            )
            .await;

        // Register a new subscriber with the same track
        let result = track_manager
            .set_subscriber(
                track_namespace,
                subscriber_session_id_2,
                track_id,
                track_name,
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_subscriber() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_id = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_id, track_name)
            .await;

        let result = track_manager
            .delete_subscriber(track_namespace, track_name, subscriber_session_id)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_subscriber_session_id_by_track_id() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_id = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_id, track_name)
            .await;

        let result = track_manager
            .get_subscriber_session_id_by_track_id(track_id)
            .await;

        assert_eq!(result, Some(subscriber_session_id));
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::track_manager::{track_manager, TrackManager, TrackManagerRepository};
    use crate::TrackCommand;
    use tokio::sync::mpsc;
    #[tokio::test]
    async fn set_publisher_already_exist() {
        let track_namespace = "test";
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        let result = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_publisher_not_found() {
        let track_namespace = "test";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());

        let result = track_manager.delete_publisher(track_namespace).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_publisher_session_id_by_track_namespace_not_found() {
        let track_namespace = "test";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());

        let result = track_manager
            .get_publisher_session_id_by_track_namespace(track_namespace)
            .await;

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn set_subscriber_already_exist() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_id = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_id, track_name)
            .await;

        // Register the same subscriber
        let result = track_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_id, track_name)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn set_subscriber_track_namespace_not_found() {
        let track_namespace_1 = "test_namespace";
        let track_namespace_2 = "unexisted_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_id = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace_1, publisher_session_id)
            .await;

        // Register a new subscriber with a new track
        let result = track_manager
            .set_subscriber(
                track_namespace_2,
                subscriber_session_id,
                track_id,
                track_name,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_subscriber_subscriber_id_not_found() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id_1 = 2;
        let subscriber_session_id_2 = 3;
        let track_id = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_manager
            .set_subscriber(
                track_namespace,
                subscriber_session_id_1,
                track_id,
                track_name,
            )
            .await;

        let result = track_manager
            .delete_subscriber(track_namespace, track_name, subscriber_session_id_2)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_subscriber_track_name_not_found() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        let result = track_manager
            .delete_subscriber(track_namespace, track_name, subscriber_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_subscriber_track_namespace_not_found() {
        let track_namespace_1 = "test_namespace";
        let track_namespace_2 = "unexisted_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_id = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());
        let _ = track_manager
            .set_publisher(track_namespace_1, publisher_session_id)
            .await;
        let _ = track_manager
            .set_subscriber(
                track_namespace_1,
                subscriber_session_id,
                track_id,
                track_name,
            )
            .await;

        let result = track_manager
            .delete_subscriber(track_namespace_2, track_name, subscriber_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_subscriber_session_id_by_track_id_not_found() {
        let track_id = 3;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        let track_manager = TrackManager::new(track_tx.clone());

        let result = track_manager
            .get_subscriber_session_id_by_track_id(track_id)
            .await;

        assert_eq!(result, None);
    }
}
