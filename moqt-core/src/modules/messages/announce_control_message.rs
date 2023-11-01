use super::track_request_parameters::TrackRequestParameter;

pub(crate) struct AnnounceMessage {
    track_namespace_length: u16,
    track_namespace: String,
    track_request_parameters: TrackRequestParameter,
}
