use super::track_request_parameters::TrackRequestParameter;

pub(crate) struct SubscribeRequestMessage {
    full_track_name_length: u16,
    full_track_name: String,
    track_request_parameters: TrackRequestParameter,
}
