#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum StreamSubgroupId {
    None,
    FirstObjectIdDelta,
    Value(u64),
}

impl From<&moqt::SubgroupId> for StreamSubgroupId {
    fn from(value: &moqt::SubgroupId) -> Self {
        match value {
            moqt::SubgroupId::None => Self::None,
            moqt::SubgroupId::FirstObjectIdDelta => Self::FirstObjectIdDelta,
            moqt::SubgroupId::Value(id) => Self::Value(*id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CacheLocation {
    Stream {
        group_id: u64,
        subgroup_id: StreamSubgroupId,
        index: u64,
    },
    Datagram {
        group_id: u64,
        index: u64,
    },
}
