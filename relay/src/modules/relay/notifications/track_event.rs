use crate::modules::relay::types::StreamSubgroupId;

#[derive(Clone, Debug)]
pub(crate) enum TrackEvent {
    StreamOpened {
        group_id: u64,
        subgroup_id: StreamSubgroupId,
    },
    DatagramOpened {
        group_id: u64,
    },
    EndOfGroup,
}
