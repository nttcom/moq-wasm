pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
    pub timestamp_us: u64,
    pub duration_us: Option<u64>,
    pub codec: Option<String>,
    pub description_base64: Option<String>,
    pub avc_format: Option<String>,
}
