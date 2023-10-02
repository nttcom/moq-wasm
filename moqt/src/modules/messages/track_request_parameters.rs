pub(crate) enum TrackRequestParameter {
    GroupSequenceParameter(GroupSequenceParameter),
    ObjectSequenceParameter(ObjectSequenceParameter),
    AuthorizationInfoParameter(AuthorizationInfoParameter),
}

// for SUBSCRIBE REQUEST
pub(crate) struct GroupSequenceParameter {
    track_request_parameter_key: u8, // 0x00
    track_request_parameter_length: u8,
    track_request_parameter_value: u64,
}

// for SUBSCRIBE REQUEST
pub(crate) struct ObjectSequenceParameter {
    track_request_parameter_key: u8, // 0x01
    track_request_parameter_length: u8,
    track_request_parameter_value: u64,
}

// for SUBSCRIBE REQUEST and ANNOUNCE
pub(crate) struct AuthorizationInfoParameter {
    track_request_parameter_key: u8, // 0x02
    track_request_parameter_length: u8,
    track_request_parameter_value: String,
}
