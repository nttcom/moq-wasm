pub(crate) struct SubscribeErrorMessage {
    full_track_name_length: u16,
    full_track_name: String,
    error_code: u64,
    reason_phrase_length: u16,
    reason_phrase: String,
}
