pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

pub enum RtspPacket {
    Video(EncodedPacket),
    Audio(EncodedAudioPacket),
}

pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
    pub timestamp_us: u64,
    pub ingest_wallclock_micros: u64,
    pub pts_us: Option<u64>,
    pub dts_us: Option<u64>,
    pub duration_us: Option<u64>,
    pub codec: Option<String>,
    pub description_base64: Option<String>,
    pub avc_format: Option<String>,
}

pub struct EncodedAudioPacket {
    pub data: Vec<u8>,
    pub timestamp_us: u64,
    pub ingest_wallclock_micros: u64,
    pub pts_us: Option<u64>,
    pub dts_us: Option<u64>,
    pub duration_us: Option<u64>,
    pub codec: String,
    pub description_base64: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
}
