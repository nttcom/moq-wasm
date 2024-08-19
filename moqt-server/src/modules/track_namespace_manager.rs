use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use moqt_core::TrackNamespaceManagerRepository;
use tokio::sync::{mpsc, oneshot};
use TrackCommand::*;

type SubscriberSessionId = usize;
type TrackName = String;
type TrackNamespace = String;

#[derive(Debug, PartialEq, Clone)]
pub enum SubscriberStatus {
    Waiting,
    Activate,
}

#[derive(Debug)]
struct SubscriberObject {
    state: SubscriberStatus,
}

impl SubscriberObject {
    fn new() -> Self {
        Self {
            state: SubscriberStatus::Waiting,
        }
    }

    fn set_state(&mut self, state: SubscriberStatus) {
        self.state = state;
    }

    fn is_active(&self) -> bool {
        self.state == SubscriberStatus::Activate
    }

    fn is_waiting(&self) -> bool {
        self.state == "waiting"
    }
}

#[derive(Debug)]
struct TrackNameObject {
    track_id: Option<u64>,
    subscribers: HashMap<SubscriberSessionId, SubscriberObject>,
}

impl TrackNameObject {
    fn new() -> Self {
        Self {
            track_id: Option::None,
            subscribers: HashMap::new(),
        }
    }

    fn set_track_id(&mut self, track_id: u64) {
        self.track_id = Some(track_id);
    }

    fn is_exist_subscriber(&self, subscriber_session_id: usize) -> bool {
        self.subscribers.contains_key(&subscriber_session_id)
    }

    fn is_subscriber_empty(&self) -> bool {
        self.subscribers.is_empty()
    }

    fn set_subscriber(&mut self, subscriber_session_id: usize) {
        self.subscribers
            .insert(subscriber_session_id, SubscriberObject::new());
    }

    fn delete_subscriber(&mut self, subscriber_session_id: usize) {
        self.subscribers.remove(&subscriber_session_id);
    }
}

#[derive(Debug)]
struct TrackNamespaceObject {
    publisher_session_id: usize,
    tracks: HashMap<TrackName, TrackNameObject>,
}

impl TrackNamespaceObject {
    fn new(publisher_session_id: usize) -> Self {
        Self {
            publisher_session_id,
            tracks: HashMap::new(),
        }
    }

    fn is_exist_track_name(&self, track_name: String) -> bool {
        self.tracks.contains_key(&track_name)
    }

    fn set_track(&mut self, track_name: String) {
        self.tracks.insert(track_name, TrackNameObject::new());
    }

    fn delete_track(&mut self, track_name: String) {
        self.tracks.remove(&track_name);
    }
}

#[derive(Debug)]
struct TrackNamespaces {
    publishers: HashMap<TrackNamespace, TrackNamespaceObject>,
}

impl TrackNamespaces {
    fn new() -> Self {
        Self {
            publishers: HashMap::new(),
        }
    }

    fn is_exist_track_namespace(&self, track_namespace: String) -> bool {
        self.publishers.contains_key(&track_namespace)
    }

    fn set_publisher(
        &mut self,
        track_namespace: String,
        publisher_session_id: usize,
    ) -> Result<()> {
        if self.is_exist_track_namespace(track_namespace.clone()) {
            return Err(anyhow::anyhow!("already exist"));
        }

        let publisher = TrackNamespaceObject::new(publisher_session_id);
        self.publishers.insert(track_namespace, publisher);
        Ok(())
    }

    fn delete_publisher(&mut self, track_namespace: String) -> Result<()> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            return Err(anyhow::anyhow!("not found"));
        }

        self.publishers.remove(&track_namespace);
        Ok(())
    }

    fn get_publisher_session_id_by_track_namespace(
        &self,
        track_namespace: String,
    ) -> Option<usize> {
        self.publishers
            .get(&track_namespace)
            .map(|p| p.publisher_session_id)
    }

    fn set_subscriber(
        &mut self,
        track_namespace: String,
        subscriber_session_id: usize,
        track_name: String,
    ) -> Result<()> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            return Err(anyhow::anyhow!("track_namespace not found"));
        }
        let track_namespace_object = self.publishers.get_mut(&track_namespace).unwrap();

        if track_namespace_object.is_exist_track_name(track_name.clone()) {
            let track_name_object = track_namespace_object.tracks.get_mut(&track_name).unwrap();
            if track_name_object.is_exist_subscriber(subscriber_session_id) {
                return Err(anyhow::anyhow!("already exist"));
            }
            track_name_object.set_subscriber(subscriber_session_id);

            Ok(())
        } else {
            track_namespace_object.set_track(track_name.clone());
            let new_track_name_object = track_namespace_object.tracks.get_mut(&track_name).unwrap();
            new_track_name_object.set_subscriber(subscriber_session_id);

            Ok(())
        }
    }

    fn delete_subscriber(
        &mut self,
        track_namespace: String,
        track_name: String,
        subscriber_session_id: usize,
    ) -> Result<()> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            return Err(anyhow::anyhow!("track_namespace not found"));
        }
        let track_namespace_object = self.publishers.get_mut(&track_namespace).unwrap();

        if !track_namespace_object.is_exist_track_name(track_name.clone()) {
            return Err(anyhow::anyhow!("track_name not found"));
        }
        let track_name_object = track_namespace_object.tracks.get_mut(&track_name).unwrap();

        if !track_name_object.is_exist_subscriber(subscriber_session_id) {
            return Err(anyhow::anyhow!("subscriber not found"));
        }
        track_name_object.delete_subscriber(subscriber_session_id);
        if track_name_object.is_subscriber_empty() {
            track_namespace_object.delete_track(track_name);
        }

        Ok(())
    }

    fn get_subscriber_session_ids_by_track_namespace_and_track_name(
        &self,
        track_namespace: String,
        track_name: String,
    ) -> Option<Vec<usize>> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            return None;
        }
        let track_namespace_object = self.publishers.get_mut(&track_namespace).unwrap();
        if !track_namespace_object.is_exist_track_name(track_name.clone()) {
            return None;
        }

        let track_name_object = track_namespace_object.tracks.get_mut(&track_name).unwrap();

        let waiting_subscribers = track_name_object
            .subscribers
            .iter()
            .filter(|(_, status)| status.is_waiting());

        let waiting_subscriber_session_ids: Vec<usize> = waiting_subscribers
            .map(|(session_id, _)| *session_id)
            .collect();

        if waiting_session_ids.is_empty() {
            return None;
        }

        Some(waiting_session_ids)
    }

    fn get_subscriber_session_ids_by_track_id(&self, track_id: u64) -> Option<Vec<usize>> {
        let track = self
            .publishers
            .values()
            .flat_map(|publisher| publisher.tracks.values())
            .find(|track| track.track_id == Some(track_id))?;

        let active_subscribers = track
            .subscribers
            .iter()
            .filter(|(_, status)| status.is_active());

        let active_subscriber_session_ids: Vec<usize> = active_subscribers
            .map(|(session_id, _)| *session_id)
            .collect();

        if active_subscriber_session_ids.is_empty() {
            return None;
        }
        Some(active_subscriber_session_ids)
    }

    fn set_track_id(
        &mut self,
        track_namespace: String,
        track_name: String,
        track_id: u64,
    ) -> Result<()> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            return Err(anyhow::anyhow!("track_namespace not found"));
        }

        let track_namespace_object = self.publishers.get_mut(&track_namespace).unwrap();
        if !track_namespace_object.is_exist_track_name(track_name.clone()) {
            return Err(anyhow::anyhow!("track_name not found"));
        }

        let track_name_object = track_namespace_object.tracks.get_mut(&track_name).unwrap();
        track_name_object.set_track_id(track_id);

        Ok(())
    }

    fn set_status(
        &mut self,
        track_namespace: String,
        track_name: String,
        subscriber_session_id: usize,
        status: String,
    ) -> Result<()> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            return Err(anyhow::anyhow!("track_namespace not found"));
        }
        let track_namespace_object = self.publishers.get_mut(&track_namespace).unwrap();
        if !track_namespace_object.is_exist_track_name(track_name.clone()) {
            return Err(anyhow::anyhow!("track_name not found"));
        }

        let track_name_object = track_namespace_object.tracks.get_mut(&track_name).unwrap();
        let subscriber = track_name_object
            .subscribers
            .get_mut(&subscriber_session_id)
            .unwrap()
            .set_state(status);

        Ok(())
    }
}

// Called as a separate thread
pub(crate) async fn track_namespace_manager(rx: &mut mpsc::Receiver<TrackCommand>) {
    tracing::info!("track_namespace_manager start");

    // TrackNamespaces
    // {
    //     "publishers": {
    //       "${track_namespace}": {
    //         "publisher_session_id": "usize",
    //         "tracks": {
    //           "${track_name}": {
    //             "track_id": "Option<u64>",
    //             "subscribers": {
    //               "${subscriber_session_id}": {
    //                 "state": "SubscriberStatus"
    //               }
    //             }
    //           }
    //         }
    //       }
    //     }
    //   }
    let mut namespaces: TrackNamespaces = TrackNamespaces::new();

    while let Some(cmd) = rx.recv().await {
        tracing::info!("command received");
        match cmd {
            SetPublisher {
                track_namespace,
                publisher_session_id,
                resp,
            } => match namespaces.set_publisher(track_namespace, publisher_session_id) {
                Ok(_) => resp.send(true).unwrap(),
                Err(err) => {
                    tracing::info!("set_publisher: err: {:?}", err.to_string());
                    resp.send(false).unwrap();
                }
            },
            DeletePublisher {
                track_namespace,
                resp,
            } => match namespaces.delete_publisher(track_namespace) {
                Ok(_) => resp.send(true).unwrap(),
                Err(err) => {
                    tracing::info!("set_publisher: err: {:?}", err.to_string());
                    resp.send(false).unwrap();
                }
            },
            HasNamespace {
                track_namespace,
                resp,
            } => {
                let result = namespaces.is_exist_track_namespace(track_namespace);
                resp.send(result).unwrap();
            }
            GetPublisherSessionId {
                track_namespace,
                resp,
            } => {
                let result =
                    namespaces.get_publisher_session_id_by_track_namespace(track_namespace);
                resp.send(result).unwrap();
            }
            SetSubscliber {
                track_namespace,
                subscriber_session_id,
                track_name,
                resp,
            } => {
                match namespaces.set_subscriber(track_namespace, subscriber_session_id, track_name)
                {
                    Ok(_) => resp.send(true).unwrap(),
                    Err(err) => {
                        tracing::info!("set_subscriber: err: {:?}", err.to_string());
                        resp.send(false).unwrap();
                    }
                }
            }
            DeleteSubscliber {
                track_namespace,
                track_name,
                subscriber_session_id,
                resp,
            } => match namespaces.delete_subscriber(
                track_namespace,
                track_name,
                subscriber_session_id,
            ) {
                Ok(_) => resp.send(true).unwrap(),
                Err(err) => {
                    tracing::info!("delete_subscriber: err: {:?}", err.to_string());
                    resp.send(false).unwrap();
                }
            },
            SetTrackId {
                track_namespace,
                track_name,
                track_id,
                resp,
            } => match namespaces.set_track_id(track_namespace, track_name, track_id) {
                Ok(_) => resp.send(true).unwrap(),
                Err(err) => {
                    tracing::info!("set_track_id: err: {:?}", err.to_string());
                    resp.send(false).unwrap();
                }
            },
            SetStatus {
                track_namespace,
                track_name,
                subscriber_session_id,
                status,
                resp,
            } => match namespaces.set_status(
                track_namespace,
                track_name,
                subscriber_session_id,
                status,
            ) {
                Ok(_) => resp.send(true).unwrap(),
                Err(err) => {
                    tracing::info!("set_status: err: {:?}", err.to_string());
                    resp.send(false).unwrap();
                }
            },
            GetSubscliberSessionIdsByNamespaceAndName {
                track_namespace,
                track_name,
                resp,
            } => {
                let result = namespaces
                    .get_subscriber_session_ids_by_track_namespace_and_track_name(
                        track_namespace,
                        track_name,
                    );
                resp.send(result).unwrap();
            }
            GetSubscliberSessionIdsByTrackId { track_id, resp } => {
                let result = namespaces.get_subscriber_session_ids_by_track_id(track_id);
                resp.send(result).unwrap();
            }
        }
    }

    tracing::info!("track_namespace_manager end");
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
        track_name: String,
        resp: oneshot::Sender<bool>,
    },
    DeleteSubscliber {
        track_namespace: String,
        track_name: String,
        subscriber_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    SetTrackId {
        track_namespace: String,
        track_name: String,
        track_id: u64,
        resp: oneshot::Sender<bool>,
    },
    SetStatus {
        track_namespace: String,
        track_name: String,
        subscriber_session_id: usize,
        status: String,
        resp: oneshot::Sender<bool>,
    },
    GetSubscliberSessionIdsByNamespaceAndName {
        track_namespace: String,
        track_name: String,
        resp: oneshot::Sender<Option<Vec<usize>>>,
    },
    GetSubscliberSessionIdsByTrackId {
        track_id: u64,
        resp: oneshot::Sender<Option<Vec<usize>>>,
    },
}

// Wrapper to encapsulate channel-related operations
pub(crate) struct TrackNamespaceManager {
    tx: mpsc::Sender<TrackCommand>,
}

impl TrackNamespaceManager {
    pub fn new(tx: mpsc::Sender<TrackCommand>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl TrackNamespaceManagerRepository for TrackNamespaceManager {
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
        track_name: &str,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetSubscliber {
            track_namespace: track_namespace.to_string(),
            subscriber_session_id,
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

    async fn set_track_id(
        &self,
        track_namespace: &str,
        track_name: &str,
        track_id: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetTrackId {
            track_namespace: track_namespace.to_string(),
            track_name: track_name.to_string(),
            track_id,
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

    async fn get_subscriber_session_ids_by_track_namespace_and_track_name(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<Vec<usize>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<Vec<usize>>>();

        let cmd = TrackCommand::GetSubscliberSessionIdsByNamespaceAndName {
            track_namespace: track_namespace.to_string(),
            track_name: track_name.to_string(),
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let session_ids = resp_rx.await.unwrap();

        return session_ids;
    }

    async fn get_subscriber_session_ids_by_track_id(&self, track_id: u64) -> Option<Vec<usize>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<Vec<usize>>>();

        let cmd = TrackCommand::GetSubscliberSessionIdsByTrackId {
            track_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let session_ids = resp_rx.await.unwrap();

        return session_ids;
    }

    async fn activate_subscriber(
        &self,
        track_namespace: &str,
        track_name: &str,
        subscriber_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetStatus {
            track_namespace: track_namespace.to_string(),
            track_name: track_name.to_string(),
            subscriber_session_id,
            status: "active".to_string(),
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
}

#[cfg(test)]
mod success {
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackNamespaceManager, TrackNamespaceManagerRepository,
    };
    use crate::TrackCommand;
    use tokio::sync::mpsc;
    #[tokio::test]
    async fn set_publisher() {
        let track_namespace = "test";
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let result = track_namespace_manager
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
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        let result = track_namespace_manager
            .delete_publisher(track_namespace)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn has_namespace() {
        let track_namespace = "test";
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        let result = track_namespace_manager.has_namespace(track_namespace).await;

        assert!(result);
    }

    #[tokio::test]
    async fn get_publisher_session_id_by_track_namespace() {
        let track_namespace = "test";
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        let result = track_namespace_manager
            .get_publisher_session_id_by_track_namespace(track_namespace)
            .await;

        assert_eq!(result, Some(publisher_session_id));
    }

    #[tokio::test]
    async fn set_subscriber_track_not_existed() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        // Register a new subscriber with a new track
        let result = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_subscriber_track_already_existed() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id_1 = 1;
        let subscriber_session_id_2 = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id_1, track_name)
            .await;

        // Register a new subscriber with the same track
        let result = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id_2, track_name)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_subscriber() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;

        let result = track_namespace_manager
            .delete_subscriber(track_namespace, track_name, subscriber_session_id)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_subscriber_session_ids_by_track_id() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_ids = vec![2, 3];
        let track_id = 0;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_ids[0], track_name)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_ids[1], track_name)
            .await;
        let _ = track_namespace_manager
            .set_track_id(track_namespace, track_name, track_id)
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(track_namespace, track_name, subscriber_session_ids[0])
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(track_namespace, track_name, subscriber_session_ids[1])
            .await;

        let mut result = track_namespace_manager
            .get_subscriber_session_ids_by_track_id(track_id)
            .await
            .unwrap();

        result.sort();

        assert_eq!(result, subscriber_session_ids);
    }
}

#[cfg(test)]
mod failure {
    use crate::modules::track_namespace_manager::{
        track_namespace_manager, TrackNamespaceManager, TrackNamespaceManagerRepository,
    };
    use crate::TrackCommand;
    use tokio::sync::mpsc;
    #[tokio::test]
    async fn set_publisher_already_exist() {
        let track_namespace = "test";
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        let result = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_publisher_not_found() {
        let track_namespace = "test";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let result = track_namespace_manager
            .delete_publisher(track_namespace)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_publisher_session_id_by_track_namespace_not_found() {
        let track_namespace = "test";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let result = track_namespace_manager
            .get_publisher_session_id_by_track_namespace(track_namespace)
            .await;

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn set_subscriber_already_exist() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;

        // Register the same subscriber
        let result = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id, track_name)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn set_subscriber_track_namespace_not_found() {
        let track_namespace_1 = "test_namespace";
        let track_namespace_2 = "unexisted_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace_1, publisher_session_id)
            .await;

        // Register a new subscriber with a new track
        let result = track_namespace_manager
            .set_subscriber(track_namespace_2, subscriber_session_id, track_name)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_subscriber_subscriber_id_not_found() {
        let track_namespace = "test_namespace";
        let publisher_session_id = 1;
        let subscriber_session_id_1 = 2;
        let subscriber_session_id_2 = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace, subscriber_session_id_1, track_name)
            .await;

        let result = track_namespace_manager
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
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace, publisher_session_id)
            .await;

        let result = track_namespace_manager
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
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace_1, publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace_1, subscriber_session_id, track_name)
            .await;

        let result = track_namespace_manager
            .delete_subscriber(track_namespace_2, track_name, subscriber_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_subscriber_session_ids_by_track_id_not_found() {
        let track_id = 3;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let result = track_namespace_manager
            .get_subscriber_session_ids_by_track_id(track_id)
            .await;

        assert_eq!(result, None);
    }
}
