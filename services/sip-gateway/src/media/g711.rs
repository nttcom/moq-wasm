/// One G.711 sample per byte at 8 kHz (RFC 3551 §4.5.14), so a 20 ms packet
/// carries 160 bytes and advances the RTP timestamp by 160.
pub(crate) const SAMPLES_PER_PACKET: usize = 160;

/// The G.711 variant the SDP answer picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum G711 {
    /// μ-law, static payload type 0.
    Pcmu,
    /// A-law, static payload type 8.
    Pcma,
}

impl G711 {
    pub(crate) fn from_payload_type(payload_type: u8) -> Option<Self> {
        match payload_type {
            0 => Some(Self::Pcmu),
            8 => Some(Self::Pcma),
            _ => None,
        }
    }

    pub(crate) fn payload_type(self) -> u8 {
        match self {
            Self::Pcmu => 0,
            Self::Pcma => 8,
        }
    }

    /// The GStreamer caps of the encoded audio.
    pub(crate) fn caps(self) -> &'static str {
        match self {
            Self::Pcmu => "audio/x-mulaw,rate=(int)8000,channels=(int)1",
            Self::Pcma => "audio/x-alaw,rate=(int)8000,channels=(int)1",
        }
    }

    pub(crate) fn encoder(self) -> &'static str {
        match self {
            Self::Pcmu => "mulawenc",
            Self::Pcma => "alawenc",
        }
    }

    pub(crate) fn decoder(self) -> &'static str {
        match self {
            Self::Pcmu => "mulawdec",
            Self::Pcma => "alawdec",
        }
    }

    /// The byte that encodes a zero sample (ITU-T G.711 Tables 1 and 2).
    pub(crate) fn silence(self) -> u8 {
        match self {
            Self::Pcmu => 0xFF,
            Self::Pcma => 0xD5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_static_g711_payload_types_are_known() {
        // Act / Assert
        assert_eq!(G711::from_payload_type(0), Some(G711::Pcmu));
        assert_eq!(G711::from_payload_type(8), Some(G711::Pcma));
        assert_eq!(G711::from_payload_type(101), None);
        assert_eq!(G711::Pcma.payload_type(), 8);
    }
}
