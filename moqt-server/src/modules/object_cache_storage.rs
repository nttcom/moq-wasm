use anyhow::{bail, Result};
use moqt_core::messages::data_streams::{datagram, subgroup_stream, track_stream};
use std::{collections::HashMap, time::Duration};
use tokio::sync::{mpsc, oneshot};
use ttl_cache::TtlCache;
type CacheId = usize;
type GroupId = u64;
type SubgroupId = u64;
pub(crate) type SubgroupStreamId = (GroupId, SubgroupId);

#[derive(Eq, Hash, PartialEq, Debug, Clone)]
pub(crate) struct CacheKey {
    session_id: usize,
    subscribe_id: u64,
}

impl CacheKey {
    pub(crate) fn new(session_id: usize, subscribe_id: u64) -> Self {
        CacheKey {
            session_id,
            subscribe_id,
        }
    }

    pub(crate) fn session_id(&self) -> usize {
        self.session_id
    }

    pub(crate) fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }
}

#[derive(Debug)]
pub(crate) enum ObjectCacheStorageCommand {
    CreateDatagramCache {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<()>>,
    },
    CreateTrackStreamCache {
        cache_key: CacheKey,
        header: track_stream::Header,
        resp: oneshot::Sender<Result<()>>,
    },
    CreateSubgroupStreamCache {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        header: subgroup_stream::Header,
        resp: oneshot::Sender<Result<()>>,
    },
    ExistDatagramCache {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<bool>>,
    },
    GetTrackStreamHeader {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<track_stream::Header>>,
    },
    GetSubgroupStreamHeader {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        resp: oneshot::Sender<Result<subgroup_stream::Header>>,
    },
    SetDatagramObject {
        cache_key: CacheKey,
        datagram_object: datagram::Object,
        duration: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    SetTrackStreamObject {
        cache_key: CacheKey,
        track_stream_object: track_stream::Object,
        duration: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    SetSubgroupStreamObject {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        subgroup_stream_object: subgroup_stream::Object,
        duration: u64,
        resp: oneshot::Sender<Result<()>>,
    },
    GetAbsoluteDatagramObject {
        cache_key: CacheKey,
        group_id: u64,
        object_id: u64,
        resp: oneshot::Sender<Result<Option<(CacheId, datagram::Object)>>>,
    },
    GetAbsoluteTrackStreamObject {
        cache_key: CacheKey,
        group_id: u64,
        object_id: u64,
        resp: oneshot::Sender<Result<Option<(CacheId, track_stream::Object)>>>,
    },
    GetAbsoluteSubgroupStreamObject {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
        resp: oneshot::Sender<Result<Option<(CacheId, subgroup_stream::Object)>>>,
    },
    GetNextDatagramObject {
        cache_key: CacheKey,
        cache_id: CacheId,
        resp: oneshot::Sender<Result<Option<(CacheId, datagram::Object)>>>,
    },
    GetNextTrackStreamObject {
        cache_key: CacheKey,
        cache_id: CacheId,
        resp: oneshot::Sender<Result<Option<(CacheId, track_stream::Object)>>>,
    },
    GetNextSubgroupStreamObject {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        cache_id: CacheId,
        resp: oneshot::Sender<Result<Option<(CacheId, subgroup_stream::Object)>>>,
    },
    GetLatestDatagramObject {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<Option<(CacheId, datagram::Object)>>>,
    },
    GetLatestTrackStreamObject {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<Option<(CacheId, track_stream::Object)>>>,
    },
    GetLatestDatagramGroup {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<Option<(CacheId, datagram::Object)>>>,
    },
    GetLatestTrackStreamGroup {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<Option<(CacheId, track_stream::Object)>>>,
    },
    // Since current Forwarder is generated for each Group,
    // LatestGroup is never used for SubgroupCache.
    // Use a method to get the first object of each Group instead.
    GetFirstSubgroupStreamObject {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        resp: oneshot::Sender<Result<Option<(CacheId, subgroup_stream::Object)>>>,
    },
    GetAllSubgroupIds {
        cache_key: CacheKey,
        group_id: u64,
        resp: oneshot::Sender<Result<Vec<SubgroupId>>>,
    },
    GetLargestGroupId {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<u64>>,
    },
    GetLargestObjectId {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<u64>>,
    },
    DeleteClient {
        session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
}

#[derive(Clone)]
enum Cache {
    Datagram(DatagramCache),
    TrackStream(TrackStreamCache),
    SubgroupStream(SubgroupStreamsCache),
}

#[derive(Clone)]
struct DatagramCache {
    objects: TtlCache<CacheId, datagram::Object>,
    next_cache_id: CacheId,
}

impl DatagramCache {
    fn new(max_store_size: usize) -> Self {
        let objects = TtlCache::new(max_store_size);

        Self {
            objects,
            next_cache_id: 0,
        }
    }

    fn insert_object(&mut self, object: datagram::Object, duration: u64) {
        let ttl = Duration::from_millis(duration);
        self.objects.insert(self.next_cache_id, object, ttl);
        self.next_cache_id += 1;
    }

    fn get_absolute_object_with_cache_id(
        &mut self,
        group_id: u64,
        object_id: u64,
    ) -> Option<(CacheId, datagram::Object)> {
        self.objects.iter().find_map(|(k, v)| {
            if v.group_id() == group_id && v.object_id() == object_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    fn get_next_object_with_cache_id(
        &mut self,
        cache_id: CacheId,
    ) -> Option<(CacheId, datagram::Object)> {
        let next_cache_id = cache_id + 1;
        self.objects.iter().find_map(|(k, v)| {
            if *k == next_cache_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    fn get_latest_group_with_cache_id(&mut self) -> Option<(CacheId, datagram::Object)> {
        let latest_group_id = self
            .objects
            .iter()
            .last()
            .map(|(_, v)| v.group_id())
            .unwrap();

        let latest_group = self.objects.iter().filter_map(|(k, v)| {
            if v.group_id() == latest_group_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        });

        latest_group.min_by_key(|(k, v)| (v.object_id(), *k))
    }

    fn get_latest_object_with_cache_id(&mut self) -> Option<(CacheId, datagram::Object)> {
        self.objects.iter().last().map(|(k, v)| (*k, v.clone()))
    }

    fn get_largest_group_id(&mut self) -> u64 {
        self.objects
            .iter()
            .map(|(_, v)| v.group_id())
            .max()
            .unwrap()
    }

    fn get_largest_object_id(&mut self) -> u64 {
        let largest_group_id = self.get_largest_group_id();

        self.objects
            .iter()
            .filter_map(|(_, v)| {
                if v.group_id() == largest_group_id {
                    Some(v.object_id())
                } else {
                    None
                }
            })
            .max()
            .unwrap()
    }
}

#[derive(Clone)]
struct TrackStreamCache {
    header: track_stream::Header,
    objects: TtlCache<CacheId, track_stream::Object>,
    next_cache_id: CacheId,
}

impl TrackStreamCache {
    fn new(header: track_stream::Header, max_store_size: usize) -> Self {
        let objects = TtlCache::new(max_store_size);

        Self {
            header,
            objects,
            next_cache_id: 0,
        }
    }

    fn insert_object(&mut self, object: track_stream::Object, duration: u64) {
        let ttl = Duration::from_millis(duration);
        self.objects.insert(self.next_cache_id, object, ttl);
        self.next_cache_id += 1;
    }

    fn get_header(&self) -> track_stream::Header {
        self.header.clone()
    }

    fn get_absolute_object_with_cache_id(
        &mut self,
        group_id: u64,
        object_id: u64,
    ) -> Option<(CacheId, track_stream::Object)> {
        self.objects.iter().find_map(|(k, v)| {
            if v.group_id() == group_id && v.object_id() == object_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    fn get_next_object_with_cache_id(
        &mut self,
        cache_id: CacheId,
    ) -> Option<(CacheId, track_stream::Object)> {
        let next_cache_id = cache_id + 1;
        self.objects.iter().find_map(|(k, v)| {
            if *k == next_cache_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    fn get_latest_group_with_cache_id(&mut self) -> Option<(CacheId, track_stream::Object)> {
        let latest_group_id = self
            .objects
            .iter()
            .last()
            .map(|(_, v)| v.group_id())
            .unwrap();

        let latest_group = self.objects.iter().filter_map(|(k, v)| {
            if v.group_id() == latest_group_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        });

        latest_group.min_by_key(|(k, v)| (v.object_id(), *k))
    }

    fn get_latest_object_with_cache_id(&mut self) -> Option<(CacheId, track_stream::Object)> {
        self.objects.iter().last().map(|(k, v)| (*k, v.clone()))
    }

    fn get_largest_group_id(&mut self) -> u64 {
        self.objects
            .iter()
            .map(|(_, v)| v.group_id())
            .max()
            .unwrap()
    }

    fn get_largest_object_id(&mut self) -> u64 {
        let largest_group_id = self.get_largest_group_id();

        self.objects
            .iter()
            .filter_map(|(_, v)| {
                if v.group_id() == largest_group_id {
                    Some(v.object_id())
                } else {
                    None
                }
            })
            .max()
            .unwrap()
    }
}

#[derive(Clone)]
struct SubgroupStreamsCache {
    streams: HashMap<SubgroupStreamId, SubgroupStreamCache>,
}

impl SubgroupStreamsCache {
    fn new() -> Self {
        let streams = HashMap::new();

        Self { streams }
    }

    fn add_subgroup_stream(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
        header: subgroup_stream::Header,
        max_cache_size: usize,
    ) {
        let stream = SubgroupStreamCache::new(header, max_cache_size);
        let subgroup_stream_id = (group_id, subgroup_id);

        self.streams.insert(subgroup_stream_id, stream);
    }

    fn insert_object(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
        object: subgroup_stream::Object,
        duration: u64,
    ) {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.insert_object(object, duration);
    }

    fn get_header(&self, group_id: u64, subgroup_id: u64) -> subgroup_stream::Header {
        let subgroup_stream_id = (group_id, subgroup_id);
        self.streams
            .get(&subgroup_stream_id)
            .map(|stream| stream.get_header())
            .unwrap()
    }

    fn get_absolute_object_with_cache_id(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
    ) -> Option<(CacheId, subgroup_stream::Object)> {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.get_absolute_object_with_cache_id(object_id)
    }

    fn get_next_object_with_cache_id(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
        cache_id: CacheId,
    ) -> Option<(CacheId, subgroup_stream::Object)> {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.get_next_object_with_cache_id(cache_id)
    }

    fn get_first_object_with_cache_id(
        &mut self,
        group_id: u64,
        subgroup_id: u64,
    ) -> Option<(CacheId, subgroup_stream::Object)> {
        let subgroup_stream_id = (group_id, subgroup_id);
        let subgroup_stream_cache = self.streams.get_mut(&subgroup_stream_id).unwrap();
        subgroup_stream_cache.get_first_object_with_cache_id()
    }

    fn get_largest_group_id(&mut self) -> u64 {
        self.streams.iter().map(|((gid, _), _)| *gid).max().unwrap()
    }

    fn get_largest_object_id(&mut self) -> u64 {
        let largest_group_id = self.get_largest_group_id();
        let largest_subgroup_id = self
            .streams
            .iter()
            .filter_map(|((gid, sgid), _)| {
                if *gid == largest_group_id {
                    Some(*sgid)
                } else {
                    None
                }
            })
            .max()
            .unwrap();
        let subgroup_stream_id = (largest_group_id, largest_subgroup_id);

        self.streams
            .get_mut(&subgroup_stream_id)
            .unwrap()
            .get_largest_object_id()
    }

    fn get_all_subgroup_ids(&mut self, group_id: u64) -> Vec<SubgroupId> {
        let mut subgroup_ids: Vec<SubgroupId> = self
            .streams
            .iter()
            .filter_map(
                |((gid, sgid), _)| {
                    if *gid == group_id {
                        Some(*sgid)
                    } else {
                        None
                    }
                },
            )
            .collect();

        subgroup_ids.sort_unstable();
        subgroup_ids
    }
}

#[derive(Clone)]
struct SubgroupStreamCache {
    header: subgroup_stream::Header,
    objects: TtlCache<CacheId, subgroup_stream::Object>,
    next_cache_id: CacheId,
}

impl SubgroupStreamCache {
    fn new(header: subgroup_stream::Header, max_cache_size: usize) -> Self {
        let objects = TtlCache::new(max_cache_size);

        Self {
            header,
            objects,
            next_cache_id: 0,
        }
    }

    fn insert_object(&mut self, object: subgroup_stream::Object, duration: u64) {
        let ttl = Duration::from_millis(duration);
        self.objects.insert(self.next_cache_id, object, ttl);
        self.next_cache_id += 1;
    }

    fn get_header(&self) -> subgroup_stream::Header {
        self.header.clone()
    }

    fn get_absolute_object_with_cache_id(
        &mut self,
        object_id: u64,
    ) -> Option<(CacheId, subgroup_stream::Object)> {
        self.objects.iter().find_map(|(k, v)| {
            if v.object_id() == object_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    fn get_next_object_with_cache_id(
        &mut self,
        cache_id: CacheId,
    ) -> Option<(CacheId, subgroup_stream::Object)> {
        let next_cache_id = cache_id + 1;
        self.objects.iter().find_map(|(k, v)| {
            if *k == next_cache_id {
                Some((*k, v.clone()))
            } else {
                None
            }
        })
    }

    fn get_first_object_with_cache_id(&mut self) -> Option<(CacheId, subgroup_stream::Object)> {
        self.objects.iter().next().map(|(k, v)| (*k, v.clone()))
    }

    fn get_largest_object_id(&mut self) -> u64 {
        self.objects
            .iter()
            .map(|(_, v)| v.object_id())
            .max()
            .unwrap()
    }
}

pub(crate) async fn object_cache_storage(rx: &mut mpsc::Receiver<ObjectCacheStorageCommand>) {
    tracing::trace!("object_cache_storage start");

    // TODO: set accurate size
    let max_cache_size = 100000;

    let mut storage = HashMap::<CacheKey, Cache>::new();

    while let Some(cmd) = rx.recv().await {
        tracing::trace!("command received: {:#?}", cmd);
        match cmd {
            ObjectCacheStorageCommand::CreateDatagramCache { cache_key, resp } => {
                let datagram_cache = DatagramCache::new(max_cache_size);
                let cache = Cache::Datagram(datagram_cache);

                // Insert the DatagramCache into the ObjectCacheStorage
                storage.insert(cache_key.clone(), cache);

                resp.send(Ok(())).unwrap();
            }
            ObjectCacheStorageCommand::CreateTrackStreamCache {
                cache_key,
                header,
                resp,
            } => {
                let track_stream_cache = TrackStreamCache::new(header, max_cache_size);
                let cache = Cache::TrackStream(track_stream_cache);

                // Insert the TrackStreamCache into the ObjectCacheStorage
                storage.insert(cache_key.clone(), cache);

                resp.send(Ok(())).unwrap();
            }
            ObjectCacheStorageCommand::CreateSubgroupStreamCache {
                cache_key,
                group_id,
                subgroup_id,
                header,
                resp,
            } => {
                // If the SubgroupStreamCache does not exist, create a new cache
                let cache = storage
                    .entry(cache_key)
                    .or_insert_with(|| Cache::SubgroupStream(SubgroupStreamsCache::new()));

                let subgroup_stream_cache = match cache {
                    Cache::SubgroupStream(subgroup_stream_cache) => subgroup_stream_cache,
                    _ => unreachable!(),
                };

                // Add a new SubgroupStream to the SubgroupCache
                subgroup_stream_cache.add_subgroup_stream(
                    group_id,
                    subgroup_id,
                    header,
                    max_cache_size,
                );

                resp.send(Ok(())).unwrap();
            }
            ObjectCacheStorageCommand::ExistDatagramCache { cache_key, resp } => {
                let cache = storage.get(&cache_key);
                match cache {
                    Some(Cache::Datagram(_)) => {
                        resp.send(Ok(true)).unwrap();
                    }
                    _ => {
                        resp.send(Ok(false)).unwrap();
                    }
                }
            }
            ObjectCacheStorageCommand::GetTrackStreamHeader { cache_key, resp } => {
                let cache = storage.get(&cache_key);
                let track_stream_cache = match cache {
                    Some(Cache::TrackStream(track_stream_cache)) => track_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("track stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let header = track_stream_cache.get_header();
                resp.send(Ok(header)).unwrap();
            }
            ObjectCacheStorageCommand::GetSubgroupStreamHeader {
                cache_key,
                group_id,
                subgroup_id,
                resp,
            } => {
                let cache = storage.get(&cache_key);
                let subgroup_stream_cache = match cache {
                    Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let header = subgroup_stream_cache.get_header(group_id, subgroup_id);
                resp.send(Ok(header)).unwrap();
            }
            ObjectCacheStorageCommand::SetDatagramObject {
                cache_key,
                datagram_object,
                duration,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let datagram_cache = match cache {
                    Some(Cache::Datagram(datagram_cache)) => datagram_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("datagram cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                datagram_cache.insert_object(datagram_object, duration);
                resp.send(Ok(())).unwrap();
            }
            ObjectCacheStorageCommand::SetTrackStreamObject {
                cache_key,
                track_stream_object,
                duration,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let track_stream_cache = match cache {
                    Some(Cache::TrackStream(track_stream_cache)) => track_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("track stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                track_stream_cache.insert_object(track_stream_object, duration);
                resp.send(Ok(())).unwrap();
            }
            ObjectCacheStorageCommand::SetSubgroupStreamObject {
                cache_key,
                group_id,
                subgroup_id,
                subgroup_stream_object,
                duration,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let subgroup_streams_cache = match cache {
                    Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                subgroup_streams_cache.insert_object(
                    group_id,
                    subgroup_id,
                    subgroup_stream_object,
                    duration,
                );
                resp.send(Ok(())).unwrap();
            }
            ObjectCacheStorageCommand::GetAbsoluteDatagramObject {
                cache_key,
                group_id,
                object_id,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let datagram_cache = match cache {
                    Some(Cache::Datagram(datagram_cache)) => datagram_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("datagram cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id =
                    datagram_cache.get_absolute_object_with_cache_id(group_id, object_id);
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetAbsoluteTrackStreamObject {
                cache_key,
                group_id,
                object_id,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let track_stream_cache = match cache {
                    Some(Cache::TrackStream(track_stream_cache)) => track_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("track stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id =
                    track_stream_cache.get_absolute_object_with_cache_id(group_id, object_id);
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetAbsoluteSubgroupStreamObject {
                cache_key,
                group_id,
                subgroup_id,
                object_id,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let subgroup_streams_cache = match cache {
                    Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id = subgroup_streams_cache
                    .get_absolute_object_with_cache_id(group_id, subgroup_id, object_id);
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetNextDatagramObject {
                cache_key,
                cache_id,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let datagram_cache = match cache {
                    Some(Cache::Datagram(datagram_cache)) => datagram_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("datagram cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id = datagram_cache.get_next_object_with_cache_id(cache_id);
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetNextTrackStreamObject {
                cache_key,
                cache_id,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let track_stream_cache = match cache {
                    Some(Cache::TrackStream(track_stream_cache)) => track_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("track stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id =
                    track_stream_cache.get_next_object_with_cache_id(cache_id);
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetNextSubgroupStreamObject {
                cache_key,
                group_id,
                subgroup_id,
                cache_id,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let subgroup_streams_cache = match cache {
                    Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id = subgroup_streams_cache.get_next_object_with_cache_id(
                    group_id,
                    subgroup_id,
                    cache_id,
                );
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetLatestDatagramGroup { cache_key, resp } => {
                let cache = storage.get_mut(&cache_key);
                let datagram_cache = match cache {
                    Some(Cache::Datagram(datagram_cache)) => datagram_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("datagram cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id = datagram_cache.get_latest_group_with_cache_id();
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetLatestTrackStreamGroup { cache_key, resp } => {
                let cache = storage.get_mut(&cache_key);
                let track_stream_cache = match cache {
                    Some(Cache::TrackStream(track_stream_cache)) => track_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("track stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id = track_stream_cache.get_latest_group_with_cache_id();
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetFirstSubgroupStreamObject {
                cache_key,
                group_id,
                subgroup_id,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let subgroup_streams_cache = match cache {
                    Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id =
                    subgroup_streams_cache.get_first_object_with_cache_id(group_id, subgroup_id);
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetLatestDatagramObject { cache_key, resp } => {
                let cache = storage.get_mut(&cache_key);
                let datagram_cache = match cache {
                    Some(Cache::Datagram(datagram_cache)) => datagram_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("datagram cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id = datagram_cache.get_latest_object_with_cache_id();
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetLatestTrackStreamObject { cache_key, resp } => {
                let cache = storage.get_mut(&cache_key);
                let track_stream_cache = match cache {
                    Some(Cache::TrackStream(track_stream_cache)) => track_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("track stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let object_with_cache_id = track_stream_cache.get_latest_object_with_cache_id();
                resp.send(Ok(object_with_cache_id)).unwrap();
            }
            ObjectCacheStorageCommand::GetAllSubgroupIds {
                cache_key,
                group_id,
                resp,
            } => {
                let cache = storage.get_mut(&cache_key);
                let subgroup_streams_cache = match cache {
                    Some(Cache::SubgroupStream(subgroup_stream_cache)) => subgroup_stream_cache,
                    _ => {
                        resp.send(Err(anyhow::anyhow!("subgroup stream cache not found")))
                            .unwrap();
                        continue;
                    }
                };

                let subgroup_ids = subgroup_streams_cache.get_all_subgroup_ids(group_id);
                resp.send(Ok(subgroup_ids)).unwrap();
            }
            ObjectCacheStorageCommand::GetLargestGroupId { cache_key, resp } => {
                let cache = storage.get_mut(&cache_key);
                if let Some(cache) = cache {
                    let largest_group_id: u64 = match cache {
                        Cache::Datagram(datagram_cache) => datagram_cache.get_largest_group_id(),
                        Cache::TrackStream(track_stream_cache) => {
                            track_stream_cache.get_largest_group_id()
                        }
                        Cache::SubgroupStream(subgroup_stream_cache) => {
                            subgroup_stream_cache.get_largest_group_id()
                        }
                    };

                    resp.send(Ok(largest_group_id)).unwrap();
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            ObjectCacheStorageCommand::GetLargestObjectId { cache_key, resp } => {
                let cache = storage.get_mut(&cache_key);
                if let Some(cache) = cache {
                    let largest_object_id: u64 = match cache {
                        Cache::Datagram(datagram_cache) => datagram_cache.get_largest_object_id(),
                        Cache::TrackStream(track_stream_cache) => {
                            track_stream_cache.get_largest_object_id()
                        }
                        Cache::SubgroupStream(subgroup_stream_cache) => {
                            subgroup_stream_cache.get_largest_object_id()
                        }
                    };

                    resp.send(Ok(largest_object_id)).unwrap();
                } else {
                    resp.send(Err(anyhow::anyhow!("cache not found"))).unwrap();
                }
            }
            ObjectCacheStorageCommand::DeleteClient { session_id, resp } => {
                let keys: Vec<CacheKey> = storage.keys().cloned().collect();
                for key in keys {
                    if key.session_id() == session_id {
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

    pub(crate) async fn create_datagram_cache(&mut self, cache_key: &CacheKey) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::CreateDatagramCache {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn create_track_stream_cache(
        &mut self,
        cache_key: &CacheKey,
        header: track_stream::Header,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::CreateTrackStreamCache {
            cache_key: cache_key.clone(),
            header,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn create_subgroup_stream_cache(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
        header: subgroup_stream::Header,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::CreateSubgroupStreamCache {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            header,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(_) => Ok(()),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn exist_datagram_cache(&mut self, cache_key: &CacheKey) -> Result<bool> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<bool>>();

        let cmd = ObjectCacheStorageCommand::ExistDatagramCache {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(exist) => Ok(exist),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_track_stream_header(
        &mut self,
        cache_key: &CacheKey,
    ) -> Result<track_stream::Header> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<track_stream::Header>>();

        let cmd = ObjectCacheStorageCommand::GetTrackStreamHeader {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(header_cache) => Ok(header_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_subgroup_stream_header(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
    ) -> Result<subgroup_stream::Header> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<subgroup_stream::Header>>();

        let cmd = ObjectCacheStorageCommand::GetSubgroupStreamHeader {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(header_cache) => Ok(header_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn set_datagram_object(
        &mut self,
        cache_key: &CacheKey,
        datagram_object: datagram::Object,
        duration: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::SetDatagramObject {
            cache_key: cache_key.clone(),
            datagram_object,
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

    pub(crate) async fn set_track_stream_object(
        &mut self,
        cache_key: &CacheKey,
        track_stream_object: track_stream::Object,
        duration: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::SetTrackStreamObject {
            cache_key: cache_key.clone(),
            track_stream_object,
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

    pub(crate) async fn set_subgroup_stream_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
        subgroup_stream_object: subgroup_stream::Object,
        duration: u64,
    ) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<()>>();

        let cmd = ObjectCacheStorageCommand::SetSubgroupStreamObject {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            subgroup_stream_object,
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

    pub(crate) async fn get_absolute_datagram_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        object_id: u64,
    ) -> Result<Option<(CacheId, datagram::Object)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, datagram::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetAbsoluteDatagramObject {
            cache_key: cache_key.clone(),
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

    pub(crate) async fn get_absolute_track_stream_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        object_id: u64,
    ) -> Result<Option<(CacheId, track_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, track_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetAbsoluteTrackStreamObject {
            cache_key: cache_key.clone(),
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

    pub(crate) async fn get_absolute_subgroup_stream_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
    ) -> Result<Option<(CacheId, subgroup_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, subgroup_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetAbsoluteSubgroupStreamObject {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
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

    pub(crate) async fn get_next_datagram_object(
        &mut self,
        cache_key: &CacheKey,
        cache_id: usize,
    ) -> Result<Option<(CacheId, datagram::Object)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, datagram::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetNextDatagramObject {
            cache_key: cache_key.clone(),
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

    pub(crate) async fn get_next_track_stream_object(
        &mut self,
        cache_key: &CacheKey,
        cache_id: usize,
    ) -> Result<Option<(CacheId, track_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, track_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetNextTrackStreamObject {
            cache_key: cache_key.clone(),
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

    pub(crate) async fn get_next_subgroup_stream_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
        cache_id: usize,
    ) -> Result<Option<(CacheId, subgroup_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, subgroup_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetNextSubgroupStreamObject {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
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

    pub(crate) async fn get_latest_datagram_object(
        &mut self,
        cache_key: &CacheKey,
    ) -> Result<Option<(CacheId, datagram::Object)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, datagram::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetLatestDatagramObject {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_latest_track_stream_object(
        &mut self,
        cache_key: &CacheKey,
    ) -> Result<Option<(CacheId, track_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, track_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetLatestTrackStreamObject {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_latest_datagram_group(
        &mut self,
        cache_key: &CacheKey,
    ) -> Result<Option<(CacheId, datagram::Object)>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Option<(CacheId, datagram::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetLatestDatagramGroup {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_latest_track_stream_group(
        &mut self,
        cache_key: &CacheKey,
    ) -> Result<Option<(CacheId, track_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, track_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetLatestTrackStreamGroup {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_first_subgroup_stream_object(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
        subgroup_id: u64,
    ) -> Result<Option<(CacheId, subgroup_stream::Object)>> {
        let (resp_tx, resp_rx) =
            oneshot::channel::<Result<Option<(CacheId, subgroup_stream::Object)>>>();

        let cmd = ObjectCacheStorageCommand::GetFirstSubgroupStreamObject {
            cache_key: cache_key.clone(),
            group_id,
            subgroup_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(object_cache) => Ok(object_cache),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_all_subgroup_ids(
        &mut self,
        cache_key: &CacheKey,
        group_id: u64,
    ) -> Result<Vec<u64>> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<Vec<u64>>>();

        let cmd = ObjectCacheStorageCommand::GetAllSubgroupIds {
            cache_key: cache_key.clone(),
            group_id,
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(subgroup_ids) => Ok(subgroup_ids),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_largest_group_id(&mut self, cache_key: &CacheKey) -> Result<u64> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<u64>>();

        let cmd = ObjectCacheStorageCommand::GetLargestGroupId {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(group_id) => Ok(group_id),
            Err(err) => bail!(err),
        }
    }

    pub(crate) async fn get_largest_object_id(&mut self, cache_key: &CacheKey) -> Result<u64> {
        let (resp_tx, resp_rx) = oneshot::channel::<Result<u64>>();

        let cmd = ObjectCacheStorageCommand::GetLargestObjectId {
            cache_key: cache_key.clone(),
            resp: resp_tx,
        };

        self.tx.send(cmd).await.unwrap();

        let result = resp_rx.await.unwrap();

        match result {
            Ok(group_id) => Ok(group_id),
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

    use moqt_core::messages::data_streams::{datagram, subgroup_stream, track_stream};

    use crate::modules::object_cache_storage::{
        object_cache_storage, CacheKey, ObjectCacheStorageCommand, ObjectCacheStorageWrapper,
    };

    #[tokio::test]
    async fn create_datagram_cache() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let result = object_cache_storage.create_datagram_cache(&cache_key).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_track_stream_cache() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 2;
        let publisher_priority = 3;

        let track_stream_header =
            track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let result = object_cache_storage
            .create_track_stream_cache(&cache_key, track_stream_header)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn create_subgroup_stream_cache() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 2;
        let group_id = 3;
        let subgroup_id = 4;
        let publisher_priority = 5;

        let subgroup_stream_header = subgroup_stream::Header::new(
            subscribe_id,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        )
        .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let result = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, subgroup_stream_header)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn exist_datagram_cache() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let result = object_cache_storage.exist_datagram_cache(&cache_key).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        let result = object_cache_storage.exist_datagram_cache(&cache_key).await;

        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn get_track_stream_header() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 2;
        let publisher_priority = 3;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let header =
            track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_track_stream_cache(&cache_key, header.clone())
            .await;

        let result = object_cache_storage
            .get_track_stream_header(&cache_key)
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), header);
    }

    #[tokio::test]
    async fn get_subgroup_stream_header() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 2;
        let group_id = 3;
        let subgroup_id = 4;
        let publisher_priority = 5;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let header = subgroup_stream::Header::new(
            subscribe_id,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        )
        .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header.clone())
            .await;

        let result = object_cache_storage
            .get_subgroup_stream_header(&cache_key, group_id, subgroup_id)
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), header);
    }

    #[tokio::test]
    async fn set_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let object_id = 2;
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let object_payload = vec![1, 2, 3, 4];
        let duration = 1000;
        let datagram_object = datagram::Object::new(
            subscribe_id,
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            object_status,
            object_payload,
        )
        .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;
        let result = object_cache_storage
            .set_datagram_object(&cache_key, datagram_object, duration)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_track_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let object_id = 2;
        let group_id = 3;
        let publisher_priority = 4;
        let object_status = None;
        let object_payload = vec![1, 2, 3, 4];
        let track_stream_object =
            track_stream::Object::new(group_id, object_id, object_status, object_payload).unwrap();
        let header = track_stream::Header::new(subscribe_id, group_id, publisher_priority).unwrap();
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_track_stream_cache(&cache_key, header)
            .await;
        let result = object_cache_storage
            .set_track_stream_object(&cache_key, track_stream_object, duration)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_subgroup_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let track_alias = 2;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let object_id = 2;
        let group_id = 3;
        let subgroup_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let object_payload = vec![1, 2, 3, 4];
        let subgroup_stream_object =
            subgroup_stream::Object::new(object_id, object_status, object_payload).unwrap();
        let header = subgroup_stream::Header::new(
            subscribe_id,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        )
        .unwrap();
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
            .await;
        let result = object_cache_storage
            .set_subgroup_stream_object(
                &cache_key,
                group_id,
                subgroup_id,
                subgroup_stream_object,
                duration,
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn get_absolute_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let datagram_object = datagram::Object::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            let _ = object_cache_storage
                .set_datagram_object(&cache_key, datagram_object, duration)
                .await;
        }

        let object_id = 5;
        let expected_cache_id = 5;
        let expected_object_payload = vec![5, 6, 7, 8];
        let expected_object = datagram::Object::new(
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
            .get_absolute_datagram_object(&cache_key, group_id, object_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_absolute_track_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header =
            track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_track_stream_cache(&cache_key, header)
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let track_stream_object =
                track_stream::Object::new(group_id, object_id, object_status, object_payload)
                    .unwrap();

            let _ = object_cache_storage
                .set_track_stream_object(&cache_key, track_stream_object, duration)
                .await;
        }

        let object_id = 7;
        let expected_cache_id = 7;
        let expected_object_payload = vec![7, 8, 9, 10];
        let expected_object =
            track_stream::Object::new(group_id, object_id, object_status, expected_object_payload)
                .unwrap();

        let result = object_cache_storage
            .get_absolute_track_stream_object(&cache_key, group_id, object_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_absolute_subgroup_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let object_status = None;
        let duration = 1000;
        let header = subgroup_stream::Header::new(
            subscribe_id,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        )
        .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup_stream_object =
                subgroup_stream::Object::new(object_id, object_status, object_payload).unwrap();

            let _ = object_cache_storage
                .set_subgroup_stream_object(
                    &cache_key,
                    group_id,
                    subgroup_id,
                    subgroup_stream_object,
                    duration,
                )
                .await;
        }

        let object_id = 9;
        let expected_cache_id = 9;
        let expected_object_payload = vec![9, 10, 11, 12];
        let expected_object =
            subgroup_stream::Object::new(object_id, object_status, expected_object_payload)
                .unwrap();

        let result = object_cache_storage
            .get_absolute_subgroup_stream_object(&cache_key, group_id, subgroup_id, object_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_next_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let datagram_object = datagram::Object::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            let _ = object_cache_storage
                .set_datagram_object(&cache_key, datagram_object, duration)
                .await;
        }

        let cache_id = 2;
        let expected_object_id = 3;
        let expected_cache_id = 3;
        let expected_object_payload = vec![3, 4, 5, 6];
        let expected_object = datagram::Object::new(
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
            .get_next_datagram_object(&cache_key, cache_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_next_track_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header =
            track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_track_stream_cache(&cache_key, header)
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let track_stream_object =
                track_stream::Object::new(group_id, object_id, object_status, object_payload)
                    .unwrap();

            let _ = object_cache_storage
                .set_track_stream_object(&cache_key, track_stream_object, duration)
                .await;
        }

        let cache_id = 4;
        let expected_object_id = 5;
        let expected_cache_id = 5;
        let expected_object_payload = vec![5, 6, 7, 8];
        let expected_object = track_stream::Object::new(
            group_id,
            expected_object_id,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_next_track_stream_object(&cache_key, cache_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_next_subgroup_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let object_status = None;
        let duration = 1000;
        let header = subgroup_stream::Header::new(
            subscribe_id,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        )
        .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
            .await;

        for i in 0..10 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup_stream_object =
                subgroup_stream::Object::new(object_id, object_status, object_payload).unwrap();

            let _ = object_cache_storage
                .set_subgroup_stream_object(
                    &cache_key,
                    group_id,
                    subgroup_id,
                    subgroup_stream_object,
                    duration,
                )
                .await;
        }

        let cache_id = 0;
        let expected_object_id = 1;
        let expected_cache_id = 1;
        let expected_object_payload = vec![1, 2, 3, 4];
        let expected_object = subgroup_stream::Object::new(
            expected_object_id,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_next_subgroup_stream_object(&cache_key, group_id, subgroup_id, cache_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_latest_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        for i in 0..6 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let datagram_object = datagram::Object::new(
                subscribe_id,
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                object_status,
                object_payload,
            )
            .unwrap();

            let _ = object_cache_storage
                .set_datagram_object(&cache_key, datagram_object, duration)
                .await;
        }

        let expected_object_id = 5;
        let expected_cache_id = 5;
        let expected_object_payload = vec![5, 6, 7, 8];
        let expected_object = datagram::Object::new(
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
            .get_latest_datagram_object(&cache_key)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_latest_track_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header =
            track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_track_stream_cache(&cache_key, header)
            .await;

        for i in 0..13 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let track_stream_object =
                track_stream::Object::new(group_id, object_id, object_status, object_payload)
                    .unwrap();

            let _ = object_cache_storage
                .set_track_stream_object(&cache_key, track_stream_object, duration)
                .await;
        }

        let expected_object_id = 12;
        let expected_cache_id = 12;
        let expected_object_payload = vec![12, 13, 14, 15];
        let expected_object = track_stream::Object::new(
            group_id,
            expected_object_id,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_latest_track_stream_object(&cache_key)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_latest_group_ascending_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        let group_size = 7;
        for j in 0..4 {
            let group_id = j as u64;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let datagram_object = datagram::Object::new(
                    subscribe_id,
                    track_alias,
                    group_id,
                    object_id,
                    publisher_priority,
                    object_status,
                    object_payload,
                )
                .unwrap();

                let _ = object_cache_storage
                    .set_datagram_object(&cache_key, datagram_object, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_group_id = 3;
        let expected_object_payload = vec![21, 22, 23, 24];
        let expected_object = datagram::Object::new(
            subscribe_id,
            track_alias,
            expected_group_id,
            expected_object_id,
            publisher_priority,
            object_status,
            expected_object_payload,
        )
        .unwrap();
        let expected_cache_id = group_size * expected_group_id as u8 + expected_object_id as u8;

        let result = object_cache_storage
            .get_latest_datagram_group(&cache_key)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id as usize);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_latest_group_descending_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

        let group_size = 7;
        for j in (2..10).rev() {
            let group_id = j as u64;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let datagram_object = datagram::Object::new(
                    subscribe_id,
                    track_alias,
                    group_id,
                    object_id,
                    publisher_priority,
                    object_status,
                    object_payload,
                )
                .unwrap();

                let _ = object_cache_storage
                    .set_datagram_object(&cache_key, datagram_object, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_group_id = 2;
        let expected_cache_id = 49;
        let expected_object_payload = vec![14, 15, 16, 17];
        let expected_object = datagram::Object::new(
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
            .get_latest_datagram_group(&cache_key)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_latest_group_ascending_track_stream() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header =
            track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_track_stream_cache(&cache_key, header)
            .await;

        let group_size = 12;
        for j in 0..8 {
            let group_id = j as u64;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let track_stream_object =
                    track_stream::Object::new(group_id, object_id, object_status, object_payload)
                        .unwrap();

                let _ = object_cache_storage
                    .set_track_stream_object(&cache_key, track_stream_object, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_group_id = 7;
        let expected_object_payload = vec![84, 85, 86, 87];
        let expected_object = track_stream::Object::new(
            expected_group_id,
            expected_object_id,
            object_status,
            expected_object_payload,
        )
        .unwrap();
        let expected_cache_id = group_size * expected_group_id as u8 + expected_object_id as u8;

        let result = object_cache_storage
            .get_latest_track_stream_group(&cache_key)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id as usize);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_latest_group_descending_track_stream() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header =
            track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_track_stream_cache(&cache_key, header)
            .await;

        let group_size = 12;
        for j in (5..9).rev() {
            let group_id = j as u64;

            for i in 0..group_size {
                let object_payload: Vec<u8> = vec![
                    j * group_size + i,
                    j * group_size + i + 1,
                    j * group_size + i + 2,
                    j * group_size + i + 3,
                ];
                let object_id = i as u64;

                let track_stream_object =
                    track_stream::Object::new(group_id, object_id, object_status, object_payload)
                        .unwrap();

                let _ = object_cache_storage
                    .set_track_stream_object(&cache_key, track_stream_object, duration)
                    .await;
            }
        }

        let expected_object_id = 0;
        let expected_group_id = 5;
        let expected_cache_id = 36;
        let expected_object_payload = vec![60, 61, 62, 63];
        let expected_object = track_stream::Object::new(
            expected_group_id,
            expected_object_id,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_latest_track_stream_group(&cache_key)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_first_subgroup_stream_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let object_status = None;
        let duration = 1000;
        let header = subgroup_stream::Header::new(
            subscribe_id,
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        )
        .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
            .await;

        for i in 0..20 {
            let object_payload: Vec<u8> = vec![i, i + 1, i + 2, i + 3];
            let object_id = i as u64;

            let subgroup_stream_object =
                subgroup_stream::Object::new(object_id, object_status, object_payload).unwrap();

            let _ = object_cache_storage
                .set_subgroup_stream_object(
                    &cache_key,
                    group_id,
                    subgroup_id,
                    subgroup_stream_object,
                    duration,
                )
                .await;
        }

        let expected_object_id = 0;
        let expected_cache_id = 0;
        let expected_object_payload = vec![0, 1, 2, 3];
        let expected_object = subgroup_stream::Object::new(
            expected_object_id,
            object_status,
            expected_object_payload,
        )
        .unwrap();

        let result = object_cache_storage
            .get_first_subgroup_stream_object(&cache_key, group_id, subgroup_id)
            .await;

        assert!(result.is_ok());

        let (result_cache_id, result_object) = result.unwrap().unwrap();
        assert_eq!(result_cache_id, expected_cache_id);
        assert_eq!(result_object, expected_object);
    }

    #[tokio::test]
    async fn get_all_subgroup_ids() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let publisher_priority = 6;
        let object_status = None;
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        for i in 0..10 {
            let subgroup_id = i as u64;

            let header = subgroup_stream::Header::new(
                subscribe_id,
                track_alias,
                group_id,
                subgroup_id,
                publisher_priority,
            )
            .unwrap();

            let _ = object_cache_storage
                .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
                .await;

            let subgroup_stream_object =
                subgroup_stream::Object::new(subgroup_id, object_status, vec![]).unwrap();

            let _ = object_cache_storage
                .set_subgroup_stream_object(
                    &cache_key,
                    group_id,
                    subgroup_id,
                    subgroup_stream_object,
                    duration,
                )
                .await;
        }

        let expected_subgroup_ids: Vec<u64> = (0..10).collect();

        let result = object_cache_storage
            .get_all_subgroup_ids(&cache_key, group_id)
            .await;

        assert!(result.is_ok());

        let subgroup_ids = result.unwrap();
        assert_eq!(subgroup_ids, expected_subgroup_ids);
    }

    #[tokio::test]
    async fn get_largest_group_id_and_object_id_datagram_object() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage.create_datagram_cache(&cache_key).await;

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

                let datagram_object = datagram::Object::new(
                    subscribe_id,
                    track_alias,
                    group_id,
                    object_id,
                    publisher_priority,
                    object_status,
                    object_payload,
                )
                .unwrap();

                let _ = object_cache_storage
                    .set_datagram_object(&cache_key, datagram_object, duration)
                    .await;
            }
        }

        let expected_object_id = 6;
        let expected_group_id = 3;

        let group_result = object_cache_storage.get_largest_group_id(&cache_key).await;

        assert!(group_result.is_ok());

        let largest_group_id = group_result.unwrap();
        assert_eq!(largest_group_id, expected_group_id);

        let object_result = object_cache_storage.get_largest_object_id(&cache_key).await;

        assert!(object_result.is_ok());

        let largest_object = object_result.unwrap();
        assert_eq!(largest_object, expected_object_id);
    }

    #[tokio::test]
    async fn get_largest_group_id_and_object_id_track() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let publisher_priority = 5;
        let object_status = None;
        let duration = 1000;
        let header =
            track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_track_stream_cache(&cache_key, header)
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

                let track_stream_object =
                    track_stream::Object::new(group_id, object_id, object_status, object_payload)
                        .unwrap();

                let _ = object_cache_storage
                    .set_track_stream_object(&cache_key, track_stream_object, duration)
                    .await;
            }
        }

        let expected_object_id = 11;
        let expected_group_id = 7;

        let group_result = object_cache_storage.get_largest_group_id(&cache_key).await;

        assert!(group_result.is_ok());

        let largest_group_id = group_result.unwrap();
        assert_eq!(largest_group_id, expected_group_id);

        let object_result = object_cache_storage.get_largest_object_id(&cache_key).await;

        assert!(object_result.is_ok());

        let largest_object = object_result.unwrap();
        assert_eq!(largest_object, expected_object_id);
    }

    #[tokio::test]
    async fn get_largest_group_id_and_object_id_subgroup() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let group_id = 4;
        let subgroup_id = 5;
        let publisher_priority = 6;
        let object_status = None;
        let duration = 1000;
        let header = subgroup_stream::Header::new(
            subscribe_id,
            track_alias,
            group_id, // Group ID is fixed
            subgroup_id,
            publisher_priority,
        )
        .unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });
        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_subgroup_stream_cache(&cache_key, group_id, subgroup_id, header)
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

                let subgroup_stream_object =
                    subgroup_stream::Object::new(object_id, object_status, object_payload).unwrap();

                let _ = object_cache_storage
                    .set_subgroup_stream_object(
                        &cache_key,
                        group_id,
                        subgroup_id,
                        subgroup_stream_object,
                        duration,
                    )
                    .await;
            }
        }

        let expected_object_id = 14;
        let expected_group_id = 4;

        let group_result = object_cache_storage.get_largest_group_id(&cache_key).await;

        assert!(group_result.is_ok());

        let largest_group_id = group_result.unwrap();
        assert_eq!(largest_group_id, expected_group_id);

        let object_result = object_cache_storage.get_largest_object_id(&cache_key).await;

        assert!(object_result.is_ok());

        let largest_object = object_result.unwrap();
        assert_eq!(largest_object, expected_object_id);
    }

    #[tokio::test]
    async fn delete_client() {
        let session_id = 0;
        let subscribe_id = 1;
        let cache_key = CacheKey::new(session_id, subscribe_id);
        let track_alias = 3;
        let publisher_priority = 6;
        let header =
            track_stream::Header::new(subscribe_id, track_alias, publisher_priority).unwrap();

        // start object cache storage thread
        let (cache_tx, mut cache_rx) = mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut cache_rx).await });

        let mut object_cache_storage = ObjectCacheStorageWrapper::new(cache_tx);

        let _ = object_cache_storage
            .create_track_stream_cache(&cache_key, header.clone())
            .await;

        let delete_result = object_cache_storage.delete_client(session_id).await;

        assert!(delete_result.is_ok());

        let get_result = object_cache_storage
            .get_track_stream_header(&cache_key)
            .await;

        assert!(get_result.is_err());
    }
}
