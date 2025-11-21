#[derive(Default)]
pub struct PublisherState {
    pub subscribed_aliases: std::collections::HashSet<u64>,
    pub tracks: std::collections::HashMap<(String, String), TrackEntry>,
}

#[derive(Default)]
pub struct TrackEntry {
    pub alias: u64,
    pub subscriber_alias: Option<u64>,
    pub stream: Option<wtransport::stream::SendStream>,
    pub header_sent: bool,
}
