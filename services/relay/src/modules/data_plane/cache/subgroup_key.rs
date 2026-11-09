#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum SubgroupKey {
    Stream { group_id: u64, subgroup_id: u64 },
    Datagram { group_id: u64 },
}

impl SubgroupKey {
    pub(crate) fn of(object: &moqt::object::Object) -> Self {
        let group_id = object.location.group_id;
        match object.forwarding {
            moqt::object::ForwardingPreference::Subgroup { subgroup_id } => Self::Stream {
                group_id,
                subgroup_id,
            },
            moqt::object::ForwardingPreference::Datagram => Self::Datagram { group_id },
        }
    }

    pub(crate) fn group_id(self) -> u64 {
        match self {
            Self::Stream { group_id, .. } | Self::Datagram { group_id } => group_id,
        }
    }
}
