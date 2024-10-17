// https://www.ietf.org/archive/id/draft-mzanaty-moq-loc-03.html

pub struct LocHeader {
    sequence_number: u64,
    timestamp: u64,
}

pub struct LocVideoHeader {
    header: LocHeader,
    is_keyframe: bool,
}

pub struct LocAudioHeader {
    header: LocHeader,
    audio_level: u64,
}
