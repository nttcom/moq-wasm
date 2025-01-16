use anyhow::{bail, Result};
use moqt_core::messages::data_streams::{
    object_datagram::ObjectDatagram, object_stream_subgroup::ObjectStreamSubgroup,
    object_stream_track::ObjectStreamTrack, stream_header_subgroup::StreamHeaderSubgroup,
    stream_header_track::StreamHeaderTrack,
};
use std::{collections::HashMap, time::Duration};
use tokio::sync::{mpsc, oneshot};
use ttl_cache::TtlCache;
type CacheId = usize;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Header {
    Datagram,
    Track(StreamHeaderTrack),
    Subgroup(StreamHeaderSubgroup),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Object {
    Datagram(ObjectDatagram),
    Track(ObjectStreamTrack),
    Subgroup(ObjectStreamSubgroup),
}

#[derive(Debug)]
pub(crate) enum ObjectCacheStorageCommand {
    SetSubscription {
        session_id: usize,
        subscribe_id: u64,
        header_cache: Header,
        resp: oneshot::Sender<Result<()>>,
    },
    GetHeader {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<Header>>,
    },
    SetObject {
        session_id: usize,
        subscribe_id: u64,
        object_cache: Object,
        duration: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    GetAbsoluteObject {
        session_id: usize,
        subscribe_id: u64,
        group_id: u64,
        object_id: u64,
        resp: oneshot::Sender<Result<Option<(CacheId, Object)>>>,
    },
    GetFirstObject {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<Option<(CacheId, Object)>>>,
    },
    GetNextObject {
        session_id: usize,
        subscribe_id: u64,
        cache_id: usize,
        resp: oneshot::Sender<Result<Option<(CacheId, Object)>>>,
    },
    GetLatestObject {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<Option<(CacheId, Object)>>>,
    },
    GetLatestGroup {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<Option<(CacheId, Object)>>>,
    },
    GetLargestGroupId {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<u64>>,
    },
    GetLargestObjectId {
        session_id: usize,
        subscribe_id: u64,
        group_id: u64,
        resp: oneshot::Sender<Result<u64>>,
    },
    DeleteSubscription {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    DeleteClient {
        session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
}

#[derive(Clone)]
pub(crate) struct Cache {
    header_cache: Header,
    object_caches: TtlCache<CacheId, Object>,
}

impl Cache {
    pub(crate) fn new(header_cache: Header, store_size: usize) -> Self {
        let object_caches = TtlCache::new(store_size);

        Self {
            header_cache,
            object_caches,
        }
    }
}

pub(crate) async fn object_cache_storage(rx: &mut mpsc::Receiver<ObjectCacheStorageCommand>) {
    tracing::trace!("object_cache_storage start");
    // {
    //   "${(session_id, subscribe_id)}" : {
    //     "header_cache" : Header,
    //     "object_caches" : TtlCache<CacheId, Object>,
    //   }
    // }
    let mut storage = HashMap::<(usize, u64), Cache>::new();
    let mut cache_ids = HashMap::<(usize, u64), usize>::new();

    while let Some(cmd) = rx.recv().await {
        tracing::trace!("command received: {:#?}", cmd);
        match cmd {
            ObjectCacheStorageCommand::SetSubscription {
                session_id,
                subscribe_id,
                header_cache,
                resp,
            } => {
                // TODO: set accurate size
                let cache = Cache::new(header_cache, 1000);

                storage.insert((session_id, subscribe_id), cache);
                cache_ids.entry((session_id, subscribe_id)).or_insert(0);

                resp.send(Ok(())).unwrap();
            }
            ObjectCacheStorageCommand::GetHeader {
                session_id,
                subscribe_id,
                resp,
            } => {
                let cache = storage.get(&(session_id, subscribe_id));
                let header_cache = cache.map(|store| store.header_cache.clone());

                match header_cache {
                    Some(header_cache) => {
                        resp.send(Ok(header_cache)).unwrap();
                    }
                    None => {
                        resp.send(Err(anyhow::anyhow!("header cache not found")))
                            .unwrap();
                    }
                }
            }
            ObjectCacheStorageCommand::SetObject {
                session_id,
                subscribe_id,
                object_cache,
                duration,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    let id = *cache_ids.get(&(session_id, subscribe_id)).unwrap();
                    cache
                        .object_caches
                        .insert(id, object_cache, Duration::from_millis(duration));
                    *cache_ids.get_mut(&(session_id, subscribe_id)).unwrap() += 1;

                    resp.send(Ok(())).unwrap();
                } else {
                    resp.send(Err(anyhow::anyhow!("fail to object cache")))
                        .unwrap();
                }
            }
            ObjectCacheStorageCommand::GetAbsoluteObject {
                session_id,
                subscribe_id,
                group_id,
                object_id,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    // Get an object that matches the given group_id and object_id
                    let object_cache = match &cache.header_cache {
                        Header::Datagram => cache
                            .object_caches
                            .iter()
                            .find(|(_, v)| {
                                if let Object::Datagram(object) = v {
                                    object.group_id() == group_id && object.object_id() == object_id
                                } else {
                                    false
                                }
                            })
                            .map(|(k, v)| (*k, v.clone())),
                        Header::Track(_track) => cache
                            .object_caches
                            .iter()
                            .find(|(_, v)| {
                                if let Object::Track(object) = v {
                                    object.group_id() == group_id && object.object_id() == object_id
                                } else {
                                    false
                                }
                            })
                            .map(|(k, v)| (*k, v.clone())),
                        Header::Subgroup(_subgroup) => {
                            if let Header::Subgroup(subgroup) = &cache.header_cache {
                                if subgroup.group_id() != group_id {
                                    resp.send(Err(anyhow::anyhow!("cache group not matched")))
                                        .unwrap();
                                    continue;
                                }
                            }
                            cache
                                .object_caches
                                .iter()
                                .find(|(_, v)| {
                                    if let Object::Subgroup(object) = v {
                                        object.object_id() == object_id
                                    } else {
                                        false
                                    }
                                })
                                .map(|(k, v)| (*k, v.clone()))
                        }
                    };

                    match object_cache {
                        Some(object_cache) => {
                            let (id, object) = object_cache;
                            resp.send(Ok(Some((id, object)))).unwrap();
                        }
                        None => {
                            resp.send(Ok(None)).unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            ObjectCacheStorageCommand::GetFirstObject {
                session_id,
                subscribe_id,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    let mut object_caches = cache.object_caches.clone();
                    let object_cache = object_caches.iter().next().map(|(k, v)| (*k, v.clone()));

                    match object_cache {
                        Some(object_cache) => {
                            let (id, object) = object_cache;
                            resp.send(Ok(Some((id, object)))).unwrap();
                        }
                        None => {
                            resp.send(Ok(None)).unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            ObjectCacheStorageCommand::GetNextObject {
                session_id,
                subscribe_id,
                cache_id,
                resp,
            } => {
                let next_cache_id = cache_id + 1;
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    let object_cache = cache.object_caches.get(&next_cache_id).cloned();

                    match object_cache {
                        Some(object_cache) => {
                            resp.send(Ok(Some((next_cache_id, object_cache)))).unwrap();
                        }
                        None => {
                            resp.send(Ok(None)).unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            ObjectCacheStorageCommand::GetLatestGroup {
                session_id,
                subscribe_id,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    let mut object_caches = cache.object_caches.clone();

                    // Get the last group in both ascending and descending order
                    let object_cache = match &cache.header_cache {
                        // Check the group ID contained in objects and get the latest object in the latest group ID
                        Header::Datagram => {
                            let latest_group_id: Option<u64> =
                                object_caches.iter().last().map(|(_, v)| match v {
                                    Object::Datagram(object) => object.group_id(),
                                    _ => 0,
                                });

                            let latest_group = object_caches.iter().filter_map(|(k, v)| {
                                if let Object::Datagram(object) = v {
                                    if object.group_id() == latest_group_id.unwrap() {
                                        Some((k, object.object_id(), (*v).clone()))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            });

                            // Return the object with the smallest object ID within the latest group
                            latest_group
                                .min_by_key(|(_, object_id, _)| *object_id)
                                .map(|(k, _, v)| (*k, v))
                        }
                        // Check the group ID contained in objects and get the latest object in the latest group ID
                        Header::Track(_track) => {
                            let latest_group_id: Option<u64> =
                                object_caches.iter().last().map(|(_, v)| match v {
                                    Object::Track(object) => object.group_id(),
                                    _ => 0,
                                });

                            let latest_group = object_caches.iter().filter_map(|(k, v)| {
                                if let Object::Track(object) = v {
                                    if object.group_id() == latest_group_id.unwrap() {
                                        Some((k, object.object_id(), (*v).clone()))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            });

                            // Return the object with the smallest object ID within the latest group
                            latest_group
                                .min_by_key(|(_, object_id, _)| *object_id)
                                .map(|(k, _, v)| (*k, v))
                        }
                        // Get the latest object because the group ID is the same in the subgroup
                        Header::Subgroup(_subgroup) => {
                            object_caches.iter().next().map(|(k, v)| (*k, v.clone()))
                        }
                    };

                    match object_cache {
                        Some(object_cache) => {
                            let (id, object) = object_cache;
                            resp.send(Ok(Some((id, object)))).unwrap();
                        }
                        None => {
                            resp.send(Ok(None)).unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            ObjectCacheStorageCommand::GetLatestObject {
                session_id,
                subscribe_id,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    let mut object_caches = cache.object_caches.clone();
                    let object_cache = object_caches.iter().last().map(|(k, v)| (*k, v.clone()));

                    match object_cache {
                        Some(object_cache) => {
                            let (id, object) = object_cache;
                            resp.send(Ok(Some((id, object)))).unwrap();
                        }
                        None => {
                            resp.send(Ok(None)).unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            ObjectCacheStorageCommand::GetLargestGroupId {
                session_id,
                subscribe_id,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    let mut object_caches = cache.object_caches.clone();

                    // It is not decided whether the group ID is ascending or descending,
                    // so it is necessary to get the maximum value
                    let largest_group_id: Option<u64> = match &cache.header_cache {
                        Header::Datagram => {
                            let max_group_id = object_caches
                                .iter()
                                .map(|(_, v)| match v {
                                    Object::Datagram(object) => object.group_id(),
                                    _ => 0,
                                })
                                .max();

                            max_group_id
                        }
                        Header::Track(_header) => {
                            let max_group_id = object_caches
                                .iter()
                                .map(|(_, v)| match v {
                                    Object::Track(object) => object.group_id(),
                                    _ => 0,
                                })
                                .max();

                            max_group_id
                        }
                        Header::Subgroup(header) => Some(header.group_id()),
                    };

                    match largest_group_id {
                        Some(largest_group_id) => {
                            resp.send(Ok(largest_group_id)).unwrap();
                        }
                        None => {
                            resp.send(Err(anyhow::anyhow!("group_id not found")))
                                .unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            ObjectCacheStorageCommand::GetLargestObjectId {
                session_id,
                subscribe_id,
                group_id,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    let mut object_caches = cache.object_caches.clone();

                    // Get the maximum object ID in the group
                    let largest_object_id: Option<u64> = match &cache.header_cache {
                        Header::Datagram => object_caches
                            .iter()
                            .filter_map(|(_, v)| match v {
                                Object::Datagram(object) => {
                                    if object.group_id() == group_id {
                                        Some(object.object_id())
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            })
                            .max(),
                        Header::Track(_header) => object_caches
                            .iter()
                            .filter_map(|(_, v)| match v {
                                Object::Track(object) => {
                                    if object.group_id() == group_id {
                                        Some(object.object_id())
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            })
                            .max(),
                        Header::Subgroup(_header) => object_caches
                            .iter()
                            .map(|(_, v)| match v {
                                Object::Subgroup(object) => object.object_id(),
                                _ => 0,
                            })
                            .max(),
                    };

                    match largest_object_id {
                        Some(largest_object_id) => {
                            resp.send(Ok(largest_object_id)).unwrap();
                        }
                        None => {
                            resp.send(Err(anyhow::anyhow!("object_id not found")))
                                .unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            ObjectCacheStorageCommand::DeleteSubscription {
                session_id,
                subscribe_id,
                resp,
            } => {
                let _ = storage.remove(&(session_id, subscribe_id));
                resp.send(Ok(())).unwrap();
            }
            ObjectCacheStorageCommand::DeleteClient { session_id, resp } => {
                let keys: Vec<(usize, u64)> = storage.keys().cloned().collect();
                for key in keys {
                    if key.0 == session_id {
                        let _ = storage.remove(&key);
                    }
                }
                resp.send(Ok(())).unwrap();
            }
        }
    }

    tracing::trace!("object_cache_storage end");
}

pub(crate) struct ObjectCacheStorageWrapper {
    tx: mpsc::Sender<ObjectCacheStorageCommand>,
}

impl ObjectCacheStorageWrapper {
    pub fn new(tx: mpsc::Sender<ObjectCacheStorageCommand>) -> Self {
        Self { tx }
    }

    pub(crate) async fn set_subscription(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
        header_cache: Header,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::SetSubscription {
            session_id,
            subscribe_id,
            header_cache,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_header(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
    ) -> Result<Header> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Header>>();

        let cmd = ObjectCacheStorageCommand::GetHeader {
            session_id,
            subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(header_cache) => Ok(header_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn set_object(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
        object_cache: Object,
        duration: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::SetObject {
            session_id,
            subscribe_id,
            object_cache,
            duration,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_absolute_object(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
        group_id: u64,
        object_id: u64,
    ) -> Result<Option<(CacheId, Object)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetAbsoluteObject {
            session_id,
            subscribe_id,
            group_id,
            object_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_first_object(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
    ) -> Result<Option<(CacheId, Object)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetFirstObject {
            session_id,
            subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_next_object(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
        cache_id: usize,
    ) -> Result<Option<(CacheId, Object)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetNextObject {
            session_id,
            subscribe_id,
            cache_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_latest_object(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
    ) -> Result<Option<(CacheId, Object)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetLatestObject {
            session_id,
            subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_latest_group(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
    ) -> Result<Option<(CacheId, Object)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetLatestGroup {
            session_id,
            subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_largest_group_id(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
    ) -> Result<u64> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<u64>>();

        let cmd = ObjectCacheStorageCommand::GetLargestGroupId {
            session_id,
            subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(group_id) => Ok(group_id),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_largest_object_id(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
        group_id: u64,
    ) -> Result<u64> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<u64>>();

        let cmd = ObjectCacheStorageCommand::GetLargestObjectId {
            session_id,
            subscribe_id,
            group_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(group_id) => Ok(group_id),
            Err(err) => bail!(err),
        }
    }

    async fn delete_subscription(&mut self, session_id: usize, subscribe_id: u64) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::DeleteSubscription {
            session_id,
            subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    pub async fn delete_client(&mut self, session_id: usize) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::DeleteClient {
            session_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }
}

#[cfg(test)]
mod success {
    use tokio::sync::mpsc;

    use moqt_core::messages::data_streams::{
        object_datagram::ObjectDatagram, object_stream_subgroup::ObjectStreamSubgroup,
        object_stream_track::ObjectStreamTrack, stream_header_subgroup::StreamHeaderSubgroup,
        stream_header_track::StreamHeaderTrack,
    };

    use crate::modules::object_cache_storage::{
        object_cache_storage, Header, Object, ObjectCacheStorageCommand, ObjectCacheStorageWrapper,
    };

    #[tokio::test]
    async fn set_subscription() {
        let session_id = 0;
        let subscribe_id = 1;
        let header = Header::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let result = object_cache_storage
            .set_subscription(session_id, subscribe_id, header)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_header_datagram() {
        let session_id = 0;
        let subscribe_id = 1;
        let header = Header::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        let result = object_cache_storage
            .get_header(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let header_cache = match result.unwrap() {
            Header::Datagram => Header::Datagram,
            _ => panic!("header cache not matched"),
        };
        assert_eq!(header_cache, header);
    }

    #[tokio::test]
    async fn get_header_track() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 2;
        let publisher_priority = 3;

        let stream_header_track =
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap();
        let header = Header::Track(stream_header_track.clone());

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        let result = object_cache_storage
            .get_header(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_track = match result.unwrap() {
            Header::Track(track) => track,
            _ => panic!("header cache not matched"),
        };

        assert_eq!(result_track, stream_header_track);
    }

    #[tokio::test]
    async fn get_header_subgroup() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 2;
        let group_id = 3;
        let subgroup_id = 4;
        let publisher_priority = 5;

        let stream_header_subgroup = StreamHeaderSubgroup::new(
            subscribe_id,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        )
        .unwrap();
        let header = Header::Subgroup(stream_header_subgroup.clone());

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        let result = object_cache_storage
            .get_header(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_subgroup = match result.unwrap() {
            Header::Subgroup(subgroup) => subgroup,
            _ => panic!("header cache not matched"),
        };

        assert_eq!(result_subgroup, stream_header_subgroup);
    }

    #[tokio::test]
    async fn set_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let object_id = 2;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let object_payload = vec![1, 2, 3, 4];
        let duration = 1000;
        let object_cache = Object::Datagram(
            ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap(),
        );
        let header = Header::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header)
            .await;
        let result = object_cache_storage
            .set_object(session_id, subscribe_id, object_cache, duration)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_absolute_object_datagram() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            let object_cache = Object::Datagram(datagram.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, object_cache, duration)
                .await;
        }

        let object_id = 5;
        let expected_object_payload = vec![5, 6, 7, 8];
        let expected_datagram = ObjectDatagram::new(
            subscribe_id,
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_absolute_object(session_id, subscribe_id, group_id, object_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Datagram(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_datagram);
    }

    #[tokio::test]
    async fn get_absolute_object_track() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let track =
                ObjectStreamTrack::new(group_id, object_id, object_status, object_payload).unwrap();

            let object_cache = Object::Track(track.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, object_cache, duration)
                .await;
        }

        let object_id = 7;
        let expected_object_payload = vec![7, 8, 9, 10];
        let expected_track =
            ObjectStreamTrack::new(group_id, object_id, object_status, expected_object_payload)
                .unwrap();

        let result = object_cache_storage
            .get_absolute_object(session_id, subscribe_id, group_id, object_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Track(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_track);
    }

    #[tokio::test]
    async fn get_absolute_object_subgroup() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let object_status = None;
        let duration = 1000;
        let header = Header::Subgroup(
            StreamHeaderSubgroup::new(
                subscribe_id,
                track_alias,
                group_id,
                subgroup_id,
                publisher_priority,
            )
            .unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup =
                ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

            let object_cache = Object::Subgroup(subgroup.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, object_cache, duration)
                .await;
        }

        let object_id = 9;
        let expected_object_payload = vec![9, 10, 11, 12];
        let expected_subgroup =
            ObjectStreamSubgroup::new(object_id, object_status, expected_object_payload).unwrap();

        let result = object_cache_storage
            .get_absolute_object(session_id, subscribe_id, group_id, object_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Subgroup(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_subgroup);
    }

    #[tokio::test]
    async fn get_first_object_datagram() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            let object_cache = Object::Datagram(datagram.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, object_cache, duration)
                .await;
        }

        let expected_object_id = 0;
        let expected_object_payload = vec![0, 1, 2, 3];
        let expected_datagram = ObjectDatagram::new(
            subscribe_id,
            track_alias,
            group_id,
            expected_object_id,
            publisher_priority,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_first_object(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Datagram(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_datagram);
    }

    #[tokio::test]
    async fn get_next_object_datagram() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            let object_cache = Object::Datagram(datagram.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, object_cache, duration)
                .await;
        }

        let cache_id = 2;
        let expected_object_id = 3;
        let expected_object_payload = vec![3, 4, 5, 6];
        let expected_datagram = ObjectDatagram::new(
            subscribe_id,
            track_alias,
            group_id,
            expected_object_id,
            publisher_priority,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_next_object(session_id, subscribe_id, cache_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Datagram(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_datagram);
    }

    #[tokio::test]
    async fn get_next_object_track() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let track =
                ObjectStreamTrack::new(group_id, object_id, object_status, object_payload).unwrap();

            let object_cache = Object::Track(track.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, object_cache, duration)
                .await;
        }

        let cache_id = 4;
        let expected_object_id = 5;
        let expected_object_payload = vec![5, 6, 7, 8];
        let expected_track = ObjectStreamTrack::new(
            group_id,
            expected_object_id,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_next_object(session_id, subscribe_id, cache_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Track(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_track);
    }

    #[tokio::test]
    async fn get_next_object_subgroup() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let object_status = None;
        let duration = 1000;
        let header = Header::Subgroup(
            StreamHeaderSubgroup::new(
                subscribe_id,
                track_alias,
                group_id,
                subgroup_id,
                publisher_priority,
            )
            .unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup =
                ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

            let object_cache = Object::Subgroup(subgroup.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, object_cache, duration)
                .await;
        }

        let cache_id = 0;
        let expected_object_id = 1;
        let expected_object_payload = vec![1, 2, 3, 4];
        let expected_subgroup =
            ObjectStreamSubgroup::new(expected_object_id, object_status, expected_object_payload)
                .unwrap();

        let result = object_cache_storage
            .get_next_object(session_id, subscribe_id, cache_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Subgroup(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_subgroup);
    }

    #[tokio::test]
    async fn get_latest_object_datagram() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..6 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let datagram = ObjectDatagram::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            let object_cache = Object::Datagram(datagram.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, object_cache, duration)
                .await;
        }

        let expected_object_id = 5;
        let expected_object_payload = vec![5, 6, 7, 8];
        let expected_datagram = ObjectDatagram::new(
            subscribe_id,
            track_alias,
            group_id,
            expected_object_id,
            publisher_priority,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_latest_object(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Datagram(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_datagram);
    }

    #[tokio::test]
    async fn get_latest_object_track() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..13 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let track =
                ObjectStreamTrack::new(group_id, object_id, object_status, object_payload).unwrap();

            let object_cache = Object::Track(track.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, object_cache, duration)
                .await;
        }

        let expected_object_id = 12;
        let expected_object_payload = vec![12, 13, 14, 15];
        let expected_track = ObjectStreamTrack::new(
            group_id,
            expected_object_id,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_latest_object(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Track(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_track);
    }

    #[tokio::test]
    async fn get_latest_object_subgroup() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let object_status = None;
        let duration = 1000;
        let header = Header::Subgroup(
            StreamHeaderSubgroup::new(
                subscribe_id,
                track_alias,
                group_id,
                subgroup_id,
                publisher_priority,
            )
            .unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..20 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup =
                ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

            let object_cache = Object::Subgroup(subgroup.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, object_cache, duration)
                .await;
        }

        let expected_object_id = 19;
        let expected_object_payload = vec![19, 20, 21, 22];
        let expected_subgroup =
            ObjectStreamSubgroup::new(expected_object_id, object_status, expected_object_payload)
                .unwrap();

        let result = object_cache_storage
            .get_latest_object(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Subgroup(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_subgroup);
    }

    #[tokio::test]
    async fn get_latest_group_ascending_datagram() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for j in 0..4 {
            let group_id = j as u64;
            let group_size = 7;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let datagram = ObjectDatagram::new(
                    subscribe_id,
                    track_alias,
                    group_id,
                    object_id,
                    publisher_priority,
                    object_status,
                    object_payload,
                )
                .unwrap();

                let object_cache = Object::Datagram(datagram.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, object_cache, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_group_id = 3;
        let expected_object_payload = vec![21, 22, 23, 24];
        let expected_datagram = ObjectDatagram::new(
            subscribe_id,
            track_alias,
            expected_group_id,
            expected_object_id,
            publisher_priority,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_latest_group(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Datagram(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_datagram);
    }

    #[tokio::test]
    async fn get_latest_group_descending_datagram() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for j in (2..10).rev() {
            let group_id = j as u64;
            let group_size = 7;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let datagram = ObjectDatagram::new(
                    subscribe_id,
                    track_alias,
                    group_id,
                    object_id,
                    publisher_priority,
                    object_status,
                    object_payload,
                )
                .unwrap();

                let object_cache = Object::Datagram(datagram.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, object_cache, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_group_id = 2;
        let expected_object_payload = vec![14, 15, 16, 17];
        let expected_datagram = ObjectDatagram::new(
            subscribe_id,
            track_alias,
            expected_group_id,
            expected_object_id,
            publisher_priority,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_latest_group(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Datagram(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_datagram);
    }

    #[tokio::test]
    async fn get_latest_group_ascending_track() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for j in 0..8 {
            let group_id = j as u64;
            let group_size = 12;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let track =
                    ObjectStreamTrack::new(group_id, object_id, object_status, object_payload)
                        .unwrap();

                let object_cache = Object::Track(track.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, object_cache, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_group_id = 7;
        let expected_object_payload = vec![84, 85, 86, 87];
        let expected_track = ObjectStreamTrack::new(
            expected_group_id,
            expected_object_id,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_latest_group(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Track(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_track);
    }

    #[tokio::test]
    async fn get_latest_group_descending_track() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for j in (5..9).rev() {
            let group_id = j as u64;
            let group_size = 12;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let track =
                    ObjectStreamTrack::new(group_id, object_id, object_status, object_payload)
                        .unwrap();

                let object_cache = Object::Track(track.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, object_cache, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_group_id = 5;
        let expected_object_payload = vec![60, 61, 62, 63];
        let expected_track = ObjectStreamTrack::new(
            expected_group_id,
            expected_object_id,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_latest_group(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Track(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_track);
    }

    #[tokio::test]
    async fn get_latest_group_subgroup() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let object_status = None;
        let duration = 1000;
        let header = Header::Subgroup(
            StreamHeaderSubgroup::new(
                subscribe_id,
                track_alias,
                group_id, // Group ID is fixed
                subgroup_id,
                publisher_priority,
            )
            .unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for j in 0..10 {
            let group_size = 15;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let subgroup =
                    ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

                let object_cache = Object::Subgroup(subgroup.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, object_cache, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_object_payload = vec![0, 1, 2, 3];
        let expected_subgroup =
            ObjectStreamSubgroup::new(expected_object_id, object_status, expected_object_payload)
                .unwrap();

        let result = object_cache_storage
            .get_latest_group(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_object = match result.unwrap().unwrap() {
            (_, Object::Subgroup(object)) => object,
            _ => panic!("object cache not matched"),
        };

        assert_eq!(result_object, expected_subgroup);
    }

    #[tokio::test]
    async fn get_largest_group_id_and_object_id_datagram() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for j in 0..4 {
            let group_id = j as u64;
            let group_size = 7;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let datagram = ObjectDatagram::new(
                    subscribe_id,
                    track_alias,
                    group_id,
                    object_id,
                    publisher_priority,
                    object_status,
                    object_payload,
                )
                .unwrap();

                let object_cache = Object::Datagram(datagram.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, object_cache, duration)
                    .await;
            }
        }

        let expected_object_id = 6;
        let expected_group_id = 3;

        let group_result = object_cache_storage
            .get_largest_group_id(session_id, subscribe_id)
            .await;

        assert!(group_result.is_ok());

        let largest_group_id = group_result.unwrap();
        assert_eq!(largest_group_id, expected_group_id);

        let object_result = object_cache_storage
            .get_largest_object_id(session_id, subscribe_id, largest_group_id)
            .await;

        assert!(object_result.is_ok());

        let largest_object = object_result.unwrap();
        assert_eq!(largest_object, expected_object_id);
    }

    #[tokio::test]
    async fn get_largest_group_id_and_object_id_track() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header = Header::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for j in 0..8 {
            let group_id = j as u64;
            let group_size = 12;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let track =
                    ObjectStreamTrack::new(group_id, object_id, object_status, object_payload)
                        .unwrap();

                let object_cache = Object::Track(track.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, object_cache, duration)
                    .await;
            }
        }

        let expected_object_id = 11;
        let expected_group_id = 7;

        let group_result = object_cache_storage
            .get_largest_group_id(session_id, subscribe_id)
            .await;

        assert!(group_result.is_ok());

        let largest_group_id = group_result.unwrap();
        assert_eq!(largest_group_id, expected_group_id);

        let object_result = object_cache_storage
            .get_largest_object_id(session_id, subscribe_id, largest_group_id)
            .await;

        assert!(object_result.is_ok());

        let largest_object = object_result.unwrap();
        assert_eq!(largest_object, expected_object_id);
    }

    #[tokio::test]
    async fn get_largest_group_id_and_object_id_subgroup() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let object_status = None;
        let duration = 1000;
        let header = Header::Subgroup(
            StreamHeaderSubgroup::new(
                subscribe_id,
                track_alias,
                group_id, // Group ID is fixed
                subgroup_id,
                publisher_priority,
            )
            .unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        for j in 0..10 {
            let group_size = 15;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let subgroup =
                    ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

                let object_cache = Object::Subgroup(subgroup.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, object_cache, duration)
                    .await;
            }
        }

        let expected_object_id = 14;
        let expected_group_id = 4;

        let group_result = object_cache_storage
            .get_largest_group_id(session_id, subscribe_id)
            .await;

        assert!(group_result.is_ok());

        let largest_group_id = group_result.unwrap();
        assert_eq!(largest_group_id, expected_group_id);

        let object_result = object_cache_storage
            .get_largest_object_id(session_id, subscribe_id, largest_group_id)
            .await;

        assert!(object_result.is_ok());

        let largest_object = object_result.unwrap();
        assert_eq!(largest_object, expected_object_id);
    }

    #[tokio::test]
    async fn delete_subscription() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let header = Header::Subgroup(
            StreamHeaderSubgroup::new(
                subscribe_id,
                track_alias,
                group_id,
                subgroup_id,
                publisher_priority,
            )
            .unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        let delete_result = object_cache_storage
            .delete_subscription(session_id, subscribe_id)
            .await;

        assert!(delete_result.is_ok());

        let get_result = object_cache_storage
            .get_header(session_id, subscribe_id)
            .await;

        assert!(get_result.is_err());
    }

    #[tokio::test]
    async fn delete_client() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let publisher_priority = 6;
        let header = Header::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_subscription(session_id, subscribe_id, header.clone())
            .await;

        let delete_result = object_cache_storage.delete_client(session_id).await;

        assert!(delete_result.is_ok());

        let get_result = object_cache_storage
            .get_header(session_id, subscribe_id)
            .await;

        assert!(get_result.is_err());
    }
}
