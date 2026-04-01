#[derive(Clone, Debug)]
pub(crate) enum EgressMsg {
    Object { group_id: u64, offset: u64 },
    Close { group_id: u64 },
}
