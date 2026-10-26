/// The publisher could not open the stream of the PUBLISH for a track the
/// namespace subscription for `track_namespace_prefix` covers, and will send
/// no PUBLISH for it on that subscription (draft-18 §6.1 PUBLISH_BLOCKED,
/// §10.20); a SUBSCRIBE can still reach the track. draft-14 has no such
/// message, so a draft-14 session never raises it.
#[derive(Clone, Debug)]
pub struct PublishBlockedHandler {
    pub track_namespace_prefix: String,
    pub track_namespace: String,
    pub track_name: String,
}

impl PublishBlockedHandler {
    pub(crate) fn new(prefix: &[String], suffix: &[String], track_name: String) -> Self {
        Self {
            track_namespace_prefix: prefix.join("/"),
            track_namespace: [prefix, suffix].concat().join("/"),
            track_name,
        }
    }
}
