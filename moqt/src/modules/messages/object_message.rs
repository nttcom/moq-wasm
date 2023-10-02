pub(crate) struct ObjectMessage {
    track_id: u64,
    group_sequence: u64,
    object_sequence: u64,
    object_send_order: u64,
    object_payload: Vec<u8>,
}
