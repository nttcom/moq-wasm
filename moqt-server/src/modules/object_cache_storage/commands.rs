use super::cache::{CacheId, CacheKey, SubgroupId};
use anyhow::Result;
use moqt_core::messages::data_streams::{subgroup_stream, DatagramObject};
use tokio::sync::oneshot;

#[derive(Debug)]
pub(crate) enum ObjectCacheStorageCommand {
    CreateDatagramCache {
        cache_key: CacheKey,
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
    GetSubgroupStreamHeader {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        resp: oneshot::Sender<Result<subgroup_stream::Header>>,
    },
    SetDatagramObject {
        cache_key: CacheKey,
        datagram_object: DatagramObject,
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
        resp: oneshot::Sender<Result<Option<(CacheId, DatagramObject)>>>,
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
        resp: oneshot::Sender<Result<Option<(CacheId, DatagramObject)>>>,
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
        resp: oneshot::Sender<Result<Option<(CacheId, DatagramObject)>>>,
    },
    GetLatestDatagramGroup {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<Option<(CacheId, DatagramObject)>>>,
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
    GetLatestSubgroupStreamObject {
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
