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
