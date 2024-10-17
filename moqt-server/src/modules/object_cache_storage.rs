use anyhow::{bail, Result};
use moqt_core::messages::data_streams::{
    object_datagram::ObjectDatagram, object_stream_subgroup::ObjectStreamSubgroup,
    object_stream_track::ObjectStreamTrack,
};
use moqt_core::messages::data_streams::{
    stream_header_subgroup::StreamHeaderSubgroup, stream_header_track::StreamHeaderTrack,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use ttl_cache::TtlCache;
use ObjectCacheStorageCommand::*;
type CacheId = usize;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CacheHeader {
    Datagram,
    Track(StreamHeaderTrack),
    Subgroup(StreamHeaderSubgroup),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) enum CacheObject {
    Datagram(ObjectDatagram),
    Track(ObjectStreamTrack),
    Subgroup(ObjectStreamSubgroup),
}

#[derive(Clone)]
pub(crate) struct Cache {
    cache_header: CacheHeader,
    cache_objects: Arc<Mutex<TtlCache<CacheId, CacheObject>>>,
}

impl Cache {
    pub(crate) fn new(cache_header: CacheHeader, store_size: usize) -> Self {
        let cache_objects = Arc::new(Mutex::new(TtlCache::new(store_size)));

        Self {
            cache_header,
            cache_objects,
        }
    }
}

// TODO: Remove dedicated thread and change to execute on each thread
pub(crate) async fn object_cache_storage(rx: &mut mpsc::Receiver<ObjectCacheStorageCommand>) {
    tracing::trace!("object_cache_storage start");
    // {
    //   "${(session_id, subscribe_id)}" : {
    //     "cache_header" : CacheHeader,
    //     "cache_objects" : TtlCache<CacheObject>,
    //   }
    // }
    let mut storage = HashMap::<(usize, u64), Cache>::new();
    let mut cache_ids = HashMap::<(usize, u64), usize>::new();

    while let Some(cmd) = rx.recv().await {
        tracing::debug!("command received: {:#?}", cmd);
        match cmd {
            SetChannel {
                session_id,
                subscribe_id,
                cache_header,
                resp,
            } => {
                // TODO: set accurate size
                let cache = Cache::new(cache_header, 1000);

                storage.insert((session_id, subscribe_id), cache);
                cache_ids.entry((session_id, subscribe_id)).or_insert(0);

                resp.send(Ok(())).unwrap();
            }
            GetHeader {
                session_id,
                subscribe_id,
                resp,
            } => {
                let cache = storage.get(&(session_id, subscribe_id));
                let cache_header = cache.map(|store| store.cache_header.clone());

                match cache_header {
                    Some(cache_header) => {
                        resp.send(Ok(cache_header)).unwrap();
                    }
                    None => {
                        resp.send(Err(anyhow::anyhow!("cache header not found")))
                            .unwrap();
                    }
                }
            }
            SetObject {
                session_id,
                subscribe_id,
                cache_object,
                duration,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    {
                        let id = *cache_ids.get(&(session_id, subscribe_id)).unwrap();
                        cache.cache_objects.lock().unwrap().insert(
                            id,
                            cache_object,
                            Duration::from_millis(duration),
                        );
                        *cache_ids.get_mut(&(session_id, subscribe_id)).unwrap() += 1;
                    }
                    resp.send(Ok(())).unwrap();
                } else {
                    resp.send(Err(anyhow::anyhow!("fail to cache object")))
                        .unwrap();
                }
            }
            GetAbsoluteObject {
                session_id,
                subscribe_id,
                group_id,
                object_id,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    // Get an object that matches the given group_id and object_id
                    let cache_object = match &cache.cache_header {
                        CacheHeader::Datagram => {
                            // Minimize the time to be locked
                            let mut cache_clone: TtlCache<usize, CacheObject>;
                            {
                                cache_clone = cache.cache_objects.lock().unwrap().clone();
                            }

                            cache_clone
                                .iter()
                                .find(|(_, v)| {
                                    if let CacheObject::Datagram(object) = v {
                                        object.group_id() == group_id
                                            && object.object_id() == object_id
                                    } else {
                                        false
                                    }
                                })
                                .map(|(k, v)| (*k, v.clone()))
                        }
                        CacheHeader::Track(_track) => {
                            // Minimize the time to be locked
                            let mut cache_clone: TtlCache<usize, CacheObject>;
                            {
                                cache_clone = cache.cache_objects.lock().unwrap().clone();
                            }

                            cache_clone
                                .iter()
                                .find(|(_, v)| {
                                    if let CacheObject::Track(object) = v {
                                        object.group_id() == group_id
                                            && object.object_id() == object_id
                                    } else {
                                        false
                                    }
                                })
                                .map(|(k, v)| (*k, v.clone()))
                        }
                        CacheHeader::Subgroup(_subgroup) => {
                            if let CacheHeader::Subgroup(subgroup) = &cache.cache_header {
                                if subgroup.group_id() != group_id {
                                    resp.send(Err(anyhow::anyhow!("cache group not matched")))
                                        .unwrap();
                                    continue;
                                }
                            }

                            // Minimize the time to be locked
                            let mut cache_clone: TtlCache<usize, CacheObject>;
                            {
                                cache_clone = cache.cache_objects.lock().unwrap().clone();
                            }

                            cache_clone
                                .iter()
                                .find(|(_, v)| {
                                    if let CacheObject::Subgroup(object) = v {
                                        object.object_id() == object_id
                                    } else {
                                        false
                                    }
                                })
                                .map(|(k, v)| (*k, v.clone()))
                        }
                    };

                    match cache_object {
                        Some(cache_object) => {
                            let (id, object) = cache_object;
                            resp.send(Ok((id, object))).unwrap();
                        }
                        None => {
                            resp.send(Err(anyhow::anyhow!("cache object not found")))
                                .unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            GetFirstObject {
                session_id,
                subscribe_id,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    // Minimize the time to be locked
                    let mut cache_clone: TtlCache<usize, CacheObject>;
                    {
                        cache_clone = cache.cache_objects.lock().unwrap().clone();
                    }

                    let cache_object = cache_clone.iter().next().map(|(k, v)| (*k, v.clone()));

                    match cache_object {
                        Some(cache_object) => {
                            let (id, object) = cache_object;
                            resp.send(Ok((id, object))).unwrap();
                        }
                        None => {
                            resp.send(Err(anyhow::anyhow!("cache object not found")))
                                .unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            GetNextObject {
                session_id,
                subscribe_id,
                cache_id,
                resp,
            } => {
                let next_cache_id = cache_id + 1;
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    let cache_object: Option<CacheObject>;
                    {
                        cache_object = cache
                            .cache_objects
                            .lock()
                            .unwrap()
                            .get(&next_cache_id)
                            .cloned();
                    }

                    match cache_object {
                        Some(cache_object) => {
                            resp.send(Ok((next_cache_id, cache_object))).unwrap();
                        }
                        None => {
                            resp.send(Err(anyhow::anyhow!("cache object not found")))
                                .unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            GetLatestGroup {
                session_id,
                subscribe_id,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    // Minimize the time to be locked
                    let mut cache_clone: TtlCache<usize, CacheObject>;
                    {
                        cache_clone = cache.cache_objects.lock().unwrap().clone();
                    }
                    // Get the last group in both ascending and descending order
                    let cache_object = match &cache.cache_header {
                        // Check the group ID contained in objects and get the latest object in the latest group ID
                        CacheHeader::Datagram => {
                            let latest_group_id: Option<u64> =
                                cache_clone.iter().last().map(|(_, v)| match v {
                                    CacheObject::Datagram(object) => object.group_id(),
                                    _ => 0,
                                });

                            let latest_group = cache_clone.iter().filter_map(|(k, v)| {
                                if let CacheObject::Datagram(object) = v {
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
                        CacheHeader::Track(_track) => {
                            let latest_group_id: Option<u64> =
                                cache_clone.iter().last().map(|(_, v)| match v {
                                    CacheObject::Track(object) => object.group_id(),
                                    _ => 0,
                                });

                            let latest_group = cache_clone.iter().filter_map(|(k, v)| {
                                if let CacheObject::Track(object) = v {
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
                        CacheHeader::Subgroup(_subgroup) => {
                            cache_clone.iter().next().map(|(k, v)| (*k, v.clone()))
                        }
                    };

                    match cache_object {
                        Some(cache_object) => {
                            let (id, object) = cache_object;
                            resp.send(Ok((id, object))).unwrap();
                        }
                        None => {
                            resp.send(Err(anyhow::anyhow!("cache object not found")))
                                .unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            GetLatestObject {
                session_id,
                subscribe_id,
                resp,
            } => {
                let cache = storage.get_mut(&(session_id, subscribe_id));
                if let Some(cache) = cache {
                    // Minimize the time to be locked
                    let mut cache_clone: TtlCache<usize, CacheObject>;
                    {
                        cache_clone = cache.cache_objects.lock().unwrap().clone();
                    }

                    let cache_object = cache_clone.iter().last().map(|(k, v)| (*k, v.clone()));

                    match cache_object {
                        Some(cache_object) => {
                            let (id, object) = cache_object;
                            resp.send(Ok((id, object))).unwrap();
                        }
                        None => {
                            resp.send(Err(anyhow::anyhow!("cache object not found")))
                                .unwrap();
                        }
                    }
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            DeleteChannel {
                session_id,
                subscribe_id,
                resp,
            } => {
                let _ = storage.remove(&(session_id, subscribe_id));
                resp.send(Ok(())).unwrap();
            }
        }
    }

    tracing::trace!("object_cache_storage end");
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum ObjectCacheStorageCommand {
    SetChannel {
        session_id: usize,
        subscribe_id: u64,
        cache_header: CacheHeader,
        resp: oneshot::Sender<Result<()>>,
    },
    GetHeader {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<CacheHeader>>,
    },
    SetObject {
        session_id: usize,
        subscribe_id: u64,
        cache_object: CacheObject,
        duration: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    GetAbsoluteObject {
        session_id: usize,
        subscribe_id: u64,
        group_id: u64,
        object_id: u64,
        resp: oneshot::Sender<Result<(CacheId, CacheObject)>>,
    },
    GetFirstObject {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<(CacheId, CacheObject)>>,
    },
    GetNextObject {
        session_id: usize,
        subscribe_id: u64,
        cache_id: usize,
        resp: oneshot::Sender<Result<(CacheId, CacheObject)>>,
    },
    GetLatestObject {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<(CacheId, CacheObject)>>,
    },
    GetLatestGroup {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<(CacheId, CacheObject)>>,
    },
    DeleteChannel {
        session_id: usize,
        subscribe_id: u64,
        resp: oneshot::Sender<Result<()>>,
    },
}

pub(crate) struct ObjectCacheStorageWrapper {
    tx: mpsc::Sender<ObjectCacheStorageCommand>,
}

#[allow(dead_code)]
impl ObjectCacheStorageWrapper {
    pub fn new(tx: mpsc::Sender<ObjectCacheStorageCommand>) -> Self {
        Self { tx }
    }

    pub(crate) async fn set_channel(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
        cache_header: CacheHeader,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = SetChannel {
            session_id,
            subscribe_id,
            cache_header,
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
    ) -> Result<CacheHeader> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<CacheHeader>>();

        let cmd = GetHeader {
            session_id,
            subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(cache_header) => Ok(cache_header),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn set_object(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
        cache_object: CacheObject,
        duration: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = SetObject {
            session_id,
            subscribe_id,
            cache_object,
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

    async fn get_absolute_object(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
        group_id: u64,
        object_id: u64,
    ) -> Result<(CacheId, CacheObject)> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<(CacheId, CacheObject)>>();

        let cmd = GetAbsoluteObject {
            session_id,
            subscribe_id,
            group_id,
            object_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(cache_object) => Ok(cache_object),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_first_object(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
    ) -> Result<(CacheId, CacheObject)> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<(CacheId, CacheObject)>>();

        let cmd = GetFirstObject {
            session_id,
            subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(cache_object) => Ok(cache_object),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_next_object(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
        cache_id: usize,
    ) -> Result<(CacheId, CacheObject)> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<(CacheId, CacheObject)>>();

        let cmd = GetNextObject {
            session_id,
            subscribe_id,
            cache_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(cache_object) => Ok(cache_object),
            Err(err) => bail!(err),
        }
    }

    async fn get_latest_object(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
    ) -> Result<(CacheId, CacheObject)> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<(CacheId, CacheObject)>>();

        let cmd = GetLatestObject {
            session_id,
            subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(cache_object) => Ok(cache_object),
            Err(err) => bail!(err),
        }
    }

    async fn get_latest_group(
        &mut self,
        session_id: usize,
        subscribe_id: u64,
    ) -> Result<(CacheId, CacheObject)> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<(CacheId, CacheObject)>>();

        let cmd = GetLatestGroup {
            session_id,
            subscribe_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(cache_object) => Ok(cache_object),
            Err(err) => bail!(err),
        }
    }

    async fn delete_channel(&mut self, session_id: usize, subscribe_id: u64) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = DeleteChannel {
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
}

#[cfg(test)]
mod success {
    use super::*;

    #[tokio::test]
    async fn set_channel() {
        let session_id = 0;
        let subscribe_id = 1;
        let header = CacheHeader::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let result = object_cache_storage
            .set_channel(session_id, subscribe_id, header)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_header_datagram() {
        let session_id = 0;
        let subscribe_id = 1;
        let header = CacheHeader::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
            .await;

        let result = object_cache_storage
            .get_header(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let cache_header = match result.unwrap() {
            CacheHeader::Datagram => CacheHeader::Datagram,
            _ => panic!("cache header not matched"),
        };
        assert_eq!(cache_header, header);
    }

    #[tokio::test]
    async fn get_header_track() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 2;
        let publisher_priority = 3;

        let stream_header_track =
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap();
        let header = CacheHeader::Track(stream_header_track.clone());

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
            .await;

        let result = object_cache_storage
            .get_header(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_track = match result.unwrap() {
            CacheHeader::Track(track) => track,
            _ => panic!("cache header not matched"),
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
        let header = CacheHeader::Subgroup(stream_header_subgroup.clone());

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
            .await;

        let result = object_cache_storage
            .get_header(session_id, subscribe_id)
            .await;

        assert!(result.is_ok());

        let result_subgroup = match result.unwrap() {
            CacheHeader::Subgroup(subgroup) => subgroup,
            _ => panic!("cache header not matched"),
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
        let cache_object = CacheObject::Datagram(
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
        let header = CacheHeader::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header)
            .await;
        let result = object_cache_storage
            .set_object(session_id, subscribe_id, cache_object, duration)
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
        let header = CacheHeader::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
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

            let cache_object = CacheObject::Datagram(datagram.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Datagram(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let track =
                ObjectStreamTrack::new(group_id, object_id, object_status, object_payload).unwrap();

            let cache_object = CacheObject::Track(track.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Track(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Subgroup(
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
            .set_channel(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup =
                ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

            let cache_object = CacheObject::Subgroup(subgroup.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Subgroup(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
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

            let cache_object = CacheObject::Datagram(datagram.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Datagram(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
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

            let cache_object = CacheObject::Datagram(datagram.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Datagram(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let track =
                ObjectStreamTrack::new(group_id, object_id, object_status, object_payload).unwrap();

            let cache_object = CacheObject::Track(track.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Track(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Subgroup(
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
            .set_channel(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup =
                ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

            let cache_object = CacheObject::Subgroup(subgroup.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Subgroup(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
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

            let cache_object = CacheObject::Datagram(datagram.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Datagram(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..13 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let track =
                ObjectStreamTrack::new(group_id, object_id, object_status, object_payload).unwrap();

            let cache_object = CacheObject::Track(track.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Track(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Subgroup(
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
            .set_channel(session_id, subscribe_id, header.clone())
            .await;

        for i in 0..20 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup =
                ObjectStreamSubgroup::new(object_id, object_status, object_payload).unwrap();

            let cache_object = CacheObject::Subgroup(subgroup.clone());

            let _ = object_cache_storage
                .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Subgroup(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
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

                let cache_object = CacheObject::Datagram(datagram.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Datagram(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Datagram;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
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

                let cache_object = CacheObject::Datagram(datagram.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Datagram(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
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

                let cache_object = CacheObject::Track(track.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Track(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Track(
            StreamHeaderTrack::new(subscribe_id, track_alias, publisher_priority).unwrap(),
        );

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .set_channel(session_id, subscribe_id, header.clone())
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

                let cache_object = CacheObject::Track(track.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Track(object)) => object,
            _ => panic!("cache object not matched"),
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
        let header = CacheHeader::Subgroup(
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
            .set_channel(session_id, subscribe_id, header.clone())
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

                let cache_object = CacheObject::Subgroup(subgroup.clone());

                let _ = object_cache_storage
                    .set_object(session_id, subscribe_id, cache_object, duration)
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

        let result_object = match result.unwrap() {
            (_, CacheObject::Subgroup(object)) => object,
            _ => panic!("cache object not matched"),
        };

        assert_eq!(result_object, expected_subgroup);
    }

    #[tokio::test]
    async fn delete_channel() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let header = CacheHeader::Subgroup(
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
            .set_channel(session_id, subscribe_id, header.clone())
            .await;

        let delete_result = object_cache_storage
            .delete_channel(session_id, subscribe_id)
            .await;

        assert!(delete_result.is_ok());

        let get_result = object_cache_storage
            .get_header(session_id, subscribe_id)
            .await;

        assert!(get_result.is_err());
    }
}
