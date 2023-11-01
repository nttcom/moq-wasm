pub(crate) struct AnnounceError {
    track_namespace_length: u16,
    track_namespace: String,
    error_code: u64,
    reason_phrase_length: u16,
    reason_phrase: String,
}
