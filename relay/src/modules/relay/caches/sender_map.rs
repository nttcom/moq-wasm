pub(crate) struct SenderMap {
    map: DashMap<TrackKey, tokio::sync::broadcast::Sender<LatestInfo>>,
}

impl SenderMap {
    pub(crate) fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    pub(crate) fn get_or_create(
        &self,
        track_key: TrackKey,
    ) -> tokio::sync::broadcast::Sender<LatestInfo> {
        let _ = self
            .map
            .entry(track_key)
            .or_insert_with(|| tokio::sync::broadcast::channel(16).0);
        self.map.get(&track_key).unwrap().clone()
    }

    pub(crate) fn get_receiver(
        &self,
        track_key: &TrackKey,
    ) -> Option<tokio::sync::broadcast::Receiver<LatestInfo>> {
        self.map.get(track_key).map(|sender| sender.subscribe())
    }
}
