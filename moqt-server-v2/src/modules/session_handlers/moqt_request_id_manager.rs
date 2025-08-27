pub(crate) struct MOQTRequestIdManager {
    request_id: std::sync::atomic::AtomicU64
}

impl MOQTRequestIdManager {
    pub(crate) fn get_next_request_id(&mut self) -> u64 {
        self.request_id.load(std::sync::atomic::Ordering::Relaxed)
    }
}