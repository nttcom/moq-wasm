use std::collections::HashMap;

use anyhow::{bail, Result};
use async_trait::async_trait;
use moqt_core::TrackNamespaceManagerRepository;
use tokio::sync::{mpsc, oneshot};
use TrackCommand::*;

type SubscriberSessionId = usize;
type TrackName = String;
type TrackNamespace = Vec<String>;

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
        self.state == SubscriberStatus::Waiting
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

    fn delete_subscriber_from_all_tracks(&mut self, subscriber_session_id: SubscriberSessionId) {
        for track in self.tracks.values_mut() {
            track.delete_subscriber(subscriber_session_id);
        }
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

    fn is_exist_track_namespace(&self, track_namespace: Vec<String>) -> bool {
        self.publishers.contains_key(&track_namespace)
    }

    fn is_exist_track_name(&mut self, track_namespace: Vec<String>, track_name: String) -> bool {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            return false;
        }
        let track_namespace_object = self.publishers.get_mut(&track_namespace).unwrap();

        track_namespace_object.is_exist_track_name(track_name)
    }

    fn set_publisher(
        &mut self,
        track_namespace: Vec<String>,
        publisher_session_id: usize,
    ) -> Result<()> {
        if self.is_exist_track_namespace(track_namespace.clone()) {
            bail!("already exist");
        }

        let publisher = TrackNamespaceObject::new(publisher_session_id);
        self.publishers.insert(track_namespace, publisher);
        Ok(())
    }

    fn delete_publisher_by_namespace(&mut self, track_namespace: Vec<String>) -> Result<()> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            bail!("not found");
        }

        self.publishers.remove(&track_namespace);
        Ok(())
    }

    fn delete_publisher_by_session_id(&mut self, publisher_session_id: usize) -> Result<()> {
        // Retain elements other than the specified publisher_session_id
        //   = Delete the element specified publisher_session_id
        self.publishers
            .retain(|_, namespace| namespace.publisher_session_id != publisher_session_id);
        Ok(())
    }

    fn get_publisher_session_id_by_track_namespace(
        &self,
        track_namespace: Vec<String>,
    ) -> Option<usize> {
        self.publishers
            .get(&track_namespace)
            .map(|p| p.publisher_session_id)
    }

    fn set_subscriber(
        &mut self,
        track_namespace: Vec<String>,
        subscriber_session_id: usize,
        track_name: String,
    ) -> Result<()> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            bail!("track_namespace not found");
        }
        let track_namespace_object = self.publishers.get_mut(&track_namespace).unwrap();

        if track_namespace_object.is_exist_track_name(track_name.clone()) {
            let track_name_object = track_namespace_object.tracks.get_mut(&track_name).unwrap();
            if track_name_object.is_exist_subscriber(subscriber_session_id) {
                bail!("already exist");
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
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_session_id: usize,
    ) -> Result<()> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            bail!("track_namespace not found");
        }
        let track_namespace_object = self.publishers.get_mut(&track_namespace).unwrap();

        if !track_namespace_object.is_exist_track_name(track_name.clone()) {
            bail!("track_name not found");
        }
        let track_name_object = track_namespace_object.tracks.get_mut(&track_name).unwrap();

        if !track_name_object.is_exist_subscriber(subscriber_session_id) {
            bail!("subscriber not found");
        }
        track_name_object.delete_subscriber(subscriber_session_id);

        // Delete track with no subscribers
        if track_name_object.is_subscriber_empty() {
            track_namespace_object.delete_track(track_name);
        }

        Ok(())
    }

    fn delete_subscriber_from_all_publishers(
        &mut self,
        subscriber_session_id: SubscriberSessionId,
    ) -> Result<()> {
        for track_namespace_object in self.publishers.values_mut() {
            track_namespace_object.delete_subscriber_from_all_tracks(subscriber_session_id);

            // Delete track with no subscribers
            let mut empty_tracks = Vec::new();
            for (track_name, track_name_object) in track_namespace_object.tracks.iter_mut() {
                if track_name_object.is_subscriber_empty() {
                    empty_tracks.push(track_name.to_string());
                }
            }
            for track_name in empty_tracks {
                track_namespace_object.delete_track(track_name);
            }
        }

        Ok(())
    }

    fn get_subscriber_session_ids_by_track_namespace_and_track_name(
        &mut self,
        track_namespace: Vec<String>,
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

        if waiting_subscriber_session_ids.is_empty() {
            return None;
        }

        Some(waiting_subscriber_session_ids)
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
        track_namespace: Vec<String>,
        track_name: String,
        track_id: u64,
    ) -> Result<()> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            bail!("track_namespace not found");
        }

        let track_namespace_object = self.publishers.get_mut(&track_namespace).unwrap();
        if !track_namespace_object.is_exist_track_name(track_name.clone()) {
            bail!("track_name not found");
        }

        let track_name_object = track_namespace_object.tracks.get_mut(&track_name).unwrap();
        track_name_object.set_track_id(track_id);

        Ok(())
    }

    fn set_status(
        &mut self,
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_session_id: usize,
        status: SubscriberStatus,
    ) -> Result<()> {
        if !self.is_exist_track_namespace(track_namespace.clone()) {
            bail!("track_namespace not found");
        }
        let track_namespace_object = self.publishers.get_mut(&track_namespace).unwrap();
        if !track_namespace_object.is_exist_track_name(track_name.clone()) {
            bail!("track_name not found");
        }

        let track_name_object = track_namespace_object.tracks.get_mut(&track_name).unwrap();
        let subscriber = track_name_object
            .subscribers
            .get_mut(&subscriber_session_id)
            .unwrap();
        subscriber.set_state(status.clone());

        Ok(())
    }
}

// Called as a separate thread
pub(crate) async fn track_namespace_manager(rx: &mut mpsc::Receiver<TrackCommand>) {
    tracing::trace!("track_namespace_manager start");

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
        tracing::debug!("command received: {:#?}", cmd);
        match cmd {
            SetPublisher {
                track_namespace,
                publisher_session_id,
                resp,
            } => match namespaces.set_publisher(track_namespace, publisher_session_id) {
                Ok(_) => resp.send(true).unwrap(),
                Err(err) => {
                    tracing::error!("set_publisher: err: {:?}", err.to_string());
                    resp.send(false).unwrap();
                }
            },
            DeletePublisherByNamespace {
                track_namespace,
                resp,
            } => match namespaces.delete_publisher_by_namespace(track_namespace) {
                Ok(_) => resp.send(true).unwrap(),
                Err(err) => {
                    tracing::error!("delete_publisher_by_namespace: err: {:?}", err.to_string());
                    resp.send(false).unwrap();
                }
            },
            DeletePublisherBySessionId {
                publisher_session_id,
                resp,
            } => match namespaces.delete_publisher_by_session_id(publisher_session_id) {
                Ok(_) => resp.send(true).unwrap(),
                Err(err) => {
                    tracing::error!("delete_publisher_by_session_id: err: {:?}", err.to_string());
                    resp.send(false).unwrap();
                }
            },
            HasTrackNamespace {
                track_namespace,
                resp,
            } => {
                let result = namespaces.is_exist_track_namespace(track_namespace);
                resp.send(result).unwrap();
            }
            HasTrackName {
                track_namespace,
                track_name,
                resp,
            } => {
                let result = namespaces.is_exist_track_name(track_namespace, track_name);
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
                        tracing::error!("set_subscriber: err: {:?}", err.to_string());
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
                    tracing::error!("delete_subscriber: err: {:?}", err.to_string());
                    resp.send(false).unwrap();
                }
            },
            DeleteSubscliberBySessionId {
                subscriber_session_id,
                resp,
            } => match namespaces.delete_subscriber_from_all_publishers(subscriber_session_id) {
                Ok(_) => resp.send(true).unwrap(),
                Err(err) => {
                    tracing::error!(
                        "delete_subscriber_from_all_publishers: err: {:?}",
                        err.to_string()
                    );
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
                    tracing::error!("set_track_id: err: {:?}", err.to_string());
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
                    tracing::error!("set_status: err: {:?}", err.to_string());
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

    tracing::trace!("track_namespace_manager end");
}

#[derive(Debug)]
pub(crate) enum TrackCommand {
    SetPublisher {
        track_namespace: Vec<String>,
        publisher_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    DeletePublisherByNamespace {
        track_namespace: Vec<String>,
        resp: oneshot::Sender<bool>,
    },
    DeletePublisherBySessionId {
        publisher_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    HasTrackNamespace {
        track_namespace: Vec<String>,
        resp: oneshot::Sender<bool>,
    },
    HasTrackName {
        track_namespace: Vec<String>,
        track_name: String,
        resp: oneshot::Sender<bool>,
    },
    GetPublisherSessionId {
        track_namespace: Vec<String>,
        resp: oneshot::Sender<Option<usize>>,
    },
    SetSubscliber {
        track_namespace: Vec<String>,
        subscriber_session_id: usize,
        track_name: String,
        resp: oneshot::Sender<bool>,
    },
    DeleteSubscliber {
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    DeleteSubscliberBySessionId {
        subscriber_session_id: usize,
        resp: oneshot::Sender<bool>,
    },
    SetTrackId {
        track_namespace: Vec<String>,
        track_name: String,
        track_id: u64,
        resp: oneshot::Sender<bool>,
    },
    SetStatus {
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_session_id: usize,
        status: SubscriberStatus,
        resp: oneshot::Sender<bool>,
    },
    GetSubscliberSessionIdsByNamespaceAndName {
        track_namespace: Vec<String>,
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
        track_namespace: Vec<String>,
        publisher_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetPublisher {
            track_namespace,
            publisher_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("already exist"),
        }
    }

    async fn delete_publisher_by_namespace(&self, track_namespace: Vec<String>) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::DeletePublisherByNamespace {
            track_namespace,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("not found"),
        }
    }

    async fn delete_publisher_by_session_id(&self, publisher_session_id: usize) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::DeletePublisherBySessionId {
            publisher_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("unknown error"),
        }
    }

    async fn has_track_namespace(&self, track_namespace: Vec<String>) -> bool {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::HasTrackNamespace {
            track_namespace,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();
        return result;
    }

    async fn has_track_name(&self, track_namespace: Vec<String>, track_name: &str) -> bool {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::HasTrackName {
            track_namespace,
            track_name: track_name.to_string(),
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();
        return result;
    }

    async fn get_publisher_session_id_by_track_namespace(
        &self,
        track_namespace: Vec<String>,
    ) -> Option<usize> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<usize>>();
        let cmd = TrackCommand::GetPublisherSessionId {
            track_namespace,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let session_id = resp_rx.await.unwrap();

        return session_id;
    }

    async fn set_subscriber(
        &self,
        track_namespace: Vec<String>,
        subscriber_session_id: usize,
        track_name: &str,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetSubscliber {
            track_namespace,
            subscriber_session_id,
            track_name: track_name.to_string(),
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("already exist"),
        }
    }

    async fn delete_subscriber(
        &self,
        track_namespace: Vec<String>,
        track_name: &str,
        subscriber_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::DeleteSubscliber {
            track_namespace,
            track_name: track_name.to_string(),
            subscriber_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("not found"),
        }
    }

    async fn delete_subscribers_by_session_id(&self, subscriber_session_id: usize) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::DeleteSubscliberBySessionId {
            subscriber_session_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("unknown error"),
        }
    }

    async fn set_track_id(
        &self,
        track_namespace: Vec<String>,
        track_name: &str,
        track_id: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetTrackId {
            track_namespace,
            track_name: track_name.to_string(),
            track_id,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("not found"),
        }
    }

    async fn get_subscriber_session_ids_by_track_namespace_and_track_name(
        &self,
        track_namespace: Vec<String>,
        track_name: &str,
    ) -> Option<Vec<usize>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Option<Vec<usize>>>();

        let cmd = TrackCommand::GetSubscliberSessionIdsByNamespaceAndName {
            track_namespace,
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
        track_namespace: Vec<String>,
        track_name: &str,
        subscriber_session_id: usize,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<bool>();

        let cmd = TrackCommand::SetStatus {
            track_namespace,
            track_name: track_name.to_string(),
            subscriber_session_id,
            status: SubscriberStatus::Activate,
            resp: resp_tx,
        };
        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            true => Ok(()),
            false => bail!("not found"),
        }
    }

    async fn delete_client(&self, session_id: usize) -> Result<()> {
        self.delete_publisher_by_session_id(session_id).await?;
        self.delete_subscribers_by_session_id(session_id).await?;

        Ok(())
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
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
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
    async fn delete_publisher_by_namespace() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        let _ = track_namespace_manager
            .delete_publisher_by_namespace(track_namespace.clone())
            .await;

        let result = track_namespace_manager
            .get_publisher_session_id_by_track_namespace(track_namespace.clone())
            .await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn has_track_namespace() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        let result = track_namespace_manager
            .has_track_namespace(track_namespace.clone())
            .await;

        assert!(result);
    }

    #[tokio::test]
    async fn get_publisher_session_id_by_track_namespace() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        let result = track_namespace_manager
            .get_publisher_session_id_by_track_namespace(track_namespace.clone())
            .await;

        assert_eq!(result, Some(publisher_session_id));
    }

    #[tokio::test]
    async fn set_subscriber_track_not_existed() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        // Register a new subscriber with a new track
        let result = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id, track_name)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_subscriber_track_already_existed() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id_1 = 1;
        let subscriber_session_id_2 = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id_1, track_name)
            .await;

        // Register a new subscriber with the same track
        let result = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id_2, track_name)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_subscriber() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id_1 = 2;
        let subscriber_session_id_2 = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id_1, track_name)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id_2, track_name)
            .await;

        let _ = track_namespace_manager
            .delete_subscriber(track_namespace.clone(), track_name, subscriber_session_id_1)
            .await;

        let result = track_namespace_manager
            .get_subscriber_session_ids_by_track_namespace_and_track_name(
                track_namespace.clone(),
                track_name,
            )
            .await
            .unwrap();
        let expected_result = vec![subscriber_session_id_2];

        assert_eq!(result, expected_result);
    }

    #[tokio::test]
    async fn delete_last_subscriber() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id, track_name)
            .await;

        let _ = track_namespace_manager
            .delete_subscriber(track_namespace.clone(), track_name, subscriber_session_id)
            .await;

        let result = track_namespace_manager
            .has_track_name(track_namespace.clone(), track_name)
            .await;

        assert!(!result);
    }

    #[tokio::test]
    async fn get_subscriber_session_ids_by_track_id() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_ids = vec![2, 3];
        let track_id = 0;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(
                track_namespace.clone(),
                subscriber_session_ids[0],
                track_name,
            )
            .await;
        let _ = track_namespace_manager
            .set_subscriber(
                track_namespace.clone(),
                subscriber_session_ids[1],
                track_name,
            )
            .await;
        let _ = track_namespace_manager
            .set_track_id(track_namespace.clone(), track_name, track_id)
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(
                track_namespace.clone(),
                track_name,
                subscriber_session_ids[0],
            )
            .await;
        let _ = track_namespace_manager
            .activate_subscriber(
                track_namespace.clone(),
                track_name,
                subscriber_session_ids[1],
            )
            .await;

        let mut result = track_namespace_manager
            .get_subscriber_session_ids_by_track_id(track_id)
            .await
            .unwrap();

        result.sort();

        assert_eq!(result, subscriber_session_ids);
    }

    #[tokio::test]
    async fn delete_client() {
        let track_namespaces = [
            Vec::from(["test1".to_string(), "test1".to_string()]),
            Vec::from(["test2".to_string(), "test2".to_string()]),
        ];
        let publisher_session_ids = [1, 2];
        let mut subscriber_session_ids = vec![2, 3, 4];
        let track_id = 0;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        // Register:
        //   pub 1 <- sub 2, 3, 4
        //   pub 2 <- sub 3, 4
        for i in [0, 1] {
            let _ = track_namespace_manager
                .set_publisher(track_namespaces[i].clone(), publisher_session_ids[i])
                .await;
        }
        let _ = track_namespace_manager
            .set_subscriber(
                track_namespaces[0].clone(),
                subscriber_session_ids[0],
                track_name,
            )
            .await;
        for i in [0, 1] {
            let _ = track_namespace_manager
                .set_subscriber(
                    track_namespaces[i].clone(),
                    subscriber_session_ids[1],
                    track_name,
                )
                .await;
            let _ = track_namespace_manager
                .set_subscriber(
                    track_namespaces[i].clone(),
                    subscriber_session_ids[2],
                    track_name,
                )
                .await;

            let _ = track_namespace_manager
                .set_track_id(track_namespaces[i].clone(), track_name, track_id)
                .await;

            let _ = track_namespace_manager
                .activate_subscriber(
                    track_namespaces[i].clone(),
                    track_name,
                    subscriber_session_ids[1],
                )
                .await;
            let _ = track_namespace_manager
                .activate_subscriber(
                    track_namespaces[i].clone(),
                    track_name,
                    subscriber_session_ids[2],
                )
                .await;
        }
        let _ = track_namespace_manager
            .activate_subscriber(
                track_namespaces[0].clone(),
                track_name,
                subscriber_session_ids[0],
            )
            .await;

        // Delete: pub 2, sub 2
        // Remain: pub 1 <- sub 3, 4
        let _ = track_namespace_manager
            .delete_client(subscriber_session_ids[0])
            .await;

        // Test for subscriber
        // Remain: sub 3, 4
        subscriber_session_ids.remove(0);
        let mut delete_subscriber_result = track_namespace_manager
            .get_subscriber_session_ids_by_track_id(track_id)
            .await
            .unwrap();

        delete_subscriber_result.sort();

        assert_eq!(delete_subscriber_result, subscriber_session_ids);

        // Test for subscriber
        // Remain: pub 1
        let delete_publisher_result_1 = track_namespace_manager
            .get_publisher_session_id_by_track_namespace(track_namespaces[0].clone())
            .await
            .unwrap();
        let delete_publisher_result_2 = track_namespace_manager
            .get_publisher_session_id_by_track_namespace(track_namespaces[1].clone())
            .await;

        assert_eq!(delete_publisher_result_1, publisher_session_ids[0]);
        assert!(delete_publisher_result_2.is_none());
    }

    #[tokio::test]
    async fn delete_client_as_last_subscriber() {
        let track_namespaces = [
            Vec::from(["test1".to_string(), "test1".to_string()]),
            Vec::from(["test2".to_string(), "test2".to_string()]),
        ];
        let publisher_session_ids = [1, 2];
        let subscriber_session_id = 2;
        let track_id = 0;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        // Register:
        //   pub 1 <- sub 2
        //   pub 2
        for i in [0, 1] {
            let _ = track_namespace_manager
                .set_publisher(track_namespaces[i].clone(), publisher_session_ids[i])
                .await;
        }

        let _ = track_namespace_manager
            .set_subscriber(
                track_namespaces[0].clone(),
                subscriber_session_id,
                track_name,
            )
            .await;

        let _ = track_namespace_manager
            .set_track_id(track_namespaces[0].clone(), track_name, track_id)
            .await;

        let _ = track_namespace_manager
            .activate_subscriber(
                track_namespaces[0].clone(),
                track_name,
                subscriber_session_id,
            )
            .await;

        // Delete: pub 2, sub 2
        // Remain: pub 1
        let _ = track_namespace_manager
            .delete_client(subscriber_session_id)
            .await;

        let result = track_namespace_manager
            .has_track_name(track_namespaces[0].clone(), track_name)
            .await;

        assert!(!result);
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
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        let result = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_publisher_by_namespace_not_found() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());

        let result = track_namespace_manager
            .delete_publisher_by_namespace(track_namespace)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_publisher_session_id_by_track_namespace_not_found() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);

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
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id, track_name)
            .await;

        // Register the same subscriber
        let result = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id, track_name)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn set_subscriber_track_namespace_not_found() {
        let track_namespace_1 = Vec::from(["test".to_string(), "test".to_string()]);
        let track_namespace_2 = Vec::from(["unexisted".to_string(), "namespace".to_string()]);
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
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id_1 = 2;
        let subscriber_session_id_2 = 3;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace.clone(), subscriber_session_id_1, track_name)
            .await;

        let result = track_namespace_manager
            .delete_subscriber(track_namespace.clone(), track_name, subscriber_session_id_2)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_subscriber_track_name_not_found() {
        let track_namespace = Vec::from(["test".to_string(), "test".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace.clone(), publisher_session_id)
            .await;

        let result = track_namespace_manager
            .delete_subscriber(track_namespace.clone(), track_name, subscriber_session_id)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_subscriber_track_namespace_not_found() {
        let track_namespace_1 = Vec::from(["test".to_string(), "test".to_string()]);
        let track_namespace_2 = Vec::from(["unexisted".to_string(), "namespace".to_string()]);
        let publisher_session_id = 1;
        let subscriber_session_id = 2;
        let track_name = "test_name";

        // Start track management thread
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        tokio::spawn(async move { track_namespace_manager(&mut track_rx).await });

        let track_namespace_manager = TrackNamespaceManager::new(track_tx.clone());
        let _ = track_namespace_manager
            .set_publisher(track_namespace_1.clone(), publisher_session_id)
            .await;
        let _ = track_namespace_manager
            .set_subscriber(track_namespace_1.clone(), subscriber_session_id, track_name)
            .await;

        let result = track_namespace_manager
            .delete_subscriber(track_namespace_2.clone(), track_name, subscriber_session_id)
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
