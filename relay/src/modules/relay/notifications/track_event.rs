#[derive(Clone, Debug)]
pub(crate) enum TrackEvent {
    StreamOpened { group_id: u64 },
    DatagramOpened { group_id: u64 },
    LatestObject,
    EndOfGroup,
}
