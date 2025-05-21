use super::cache::{CacheKey, SubgroupId};
use anyhow::Result;
use moqt_core::messages::data_streams::{DatagramObject, subgroup_stream};
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
    HasDatagramCache {
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
    GetDatagramObject {
        cache_key: CacheKey,
        group_id: u64,
        object_id: u64,
        resp: oneshot::Sender<Result<Option<DatagramObject>>>,
    },
    GetAbsoluteOrNextSubgroupStreamObject {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
        resp: oneshot::Sender<Result<Option<subgroup_stream::Object>>>,
    },
    GetNextDatagramObject {
        cache_key: CacheKey,
        group_id: u64,
        current_object_id: u64,
        resp: oneshot::Sender<Result<Option<DatagramObject>>>,
    },
    GetNextSubgroupStreamObject {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        current_object_id: u64,
        resp: oneshot::Sender<Result<Option<subgroup_stream::Object>>>,
    },
    GetLatestDatagramObject {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<Option<DatagramObject>>>,
    },
    GetLatestDatagramGroup {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<Option<DatagramObject>>>,
    },
    // Since current Forwarder is generated for each Group,
    // LatestGroup is never used for SubgroupCache.
    // Use a method to get the first object of each Group instead.
    GetFirstSubgroupStreamObject {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        resp: oneshot::Sender<Result<Option<subgroup_stream::Object>>>,
    },
    // TODO: Remove LatestGroup since it is not exist in the draft-10
    #[allow(dead_code)]
    GetLatestSubgroupStreamObject {
        cache_key: CacheKey,
        group_id: u64,
        subgroup_id: u64,
        resp: oneshot::Sender<Result<Option<subgroup_stream::Object>>>,
    },
    GetAllSubgroupIds {
        cache_key: CacheKey,
        group_id: u64,
        resp: oneshot::Sender<Result<Vec<SubgroupId>>>,
    },
    GetLargestGroupId {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<Option<u64>>>,
    },
    GetLargestObjectId {
        cache_key: CacheKey,
        resp: oneshot::Sender<Result<Option<u64>>>,
    },
    DeleteClient {
        session_id: usize,
        resp: oneshot::Sender<Result<()>>,
    },
}
