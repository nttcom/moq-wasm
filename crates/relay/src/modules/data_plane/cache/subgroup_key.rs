#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum SubgroupKey {
    Stream { group_id: u64, subgroup_id: u64 },
    Datagram { group_id: u64 },
}

impl SubgroupKey {
    pub(crate) fn group_id(self) -> u64 {
        match self {
            Self::Stream { group_id, .. } | Self::Datagram { group_id } => group_id,
        }
    }
}
