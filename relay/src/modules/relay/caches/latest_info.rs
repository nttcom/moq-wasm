#[derive(Clone, Debug)]
pub(crate) enum LatestInfo {
    StreamOpened { group_id: u64 },
    DatagramOpened { group_id: u64 },
    LatestObject { group_id: u64, offset: u64 },
    EndOfGroup { group_id: u64 },
}
