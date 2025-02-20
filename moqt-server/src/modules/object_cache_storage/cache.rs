pub(crate) mod datagram;
pub(crate) mod subgroup_stream;
pub(crate) mod track_stream;

use datagram::DatagramCache;
use subgroup_stream::SubgroupStreamsCache;
use track_stream::TrackStreamCache;

pub(crate) type CacheId = usize;
type GroupId = u64;
pub(crate) type SubgroupId = u64;
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

#[derive(Clone)]
pub(crate) enum Cache {
    Datagram(DatagramCache),
    TrackStream(TrackStreamCache),
    SubgroupStream(SubgroupStreamsCache),
}
