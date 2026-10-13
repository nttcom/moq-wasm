use anyhow::{Result, ensure};
use bytes::{BufMut, Bytes, BytesMut};

pub const HEADER_LENGTH: usize = 12;
const VERSION: u8 = 2;
const EXTENSION_HEADER_LENGTH: usize = 4;

/// The fixed RTP header (RFC 3550 §5.1). CSRC lists, header extensions and
/// padding are consumed when parsing and never written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub marker: bool,
    pub payload_type: u8,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

impl Header {
    /// Returns the header and the payload that follows it, without the padding.
    /// Fails on a version other than 2, or on a packet shorter than its CSRC
    /// list, header extension or padding says.
    pub fn parse(datagram: &[u8]) -> Result<(Self, &[u8])> {
        ensure!(
            datagram.len() >= HEADER_LENGTH,
            "RTP packet too short: {} bytes",
            datagram.len()
        );
        let version = datagram[0] >> 6;
        ensure!(version == VERSION, "unsupported RTP version {version}");
        let has_padding = datagram[0] & 0x20 != 0;
        let has_extension = datagram[0] & 0x10 != 0;
        let csrc_count = (datagram[0] & 0x0F) as usize;
        let header = Self {
            marker: datagram[1] & 0x80 != 0,
            payload_type: datagram[1] & 0x7F,
            sequence_number: u16::from_be_bytes([datagram[2], datagram[3]]),
            timestamp: u32::from_be_bytes([datagram[4], datagram[5], datagram[6], datagram[7]]),
            ssrc: u32::from_be_bytes([datagram[8], datagram[9], datagram[10], datagram[11]]),
        };

        let mut payload_start = HEADER_LENGTH + csrc_count * 4;
        ensure!(
            payload_start <= datagram.len(),
            "RTP packet truncated in CSRC list"
        );
        if has_extension {
            ensure!(
                payload_start + EXTENSION_HEADER_LENGTH <= datagram.len(),
                "RTP packet truncated in header extension"
            );
            let words =
                u16::from_be_bytes([datagram[payload_start + 2], datagram[payload_start + 3]]);
            payload_start += EXTENSION_HEADER_LENGTH + words as usize * 4;
            ensure!(
                payload_start <= datagram.len(),
                "RTP header extension longer than the packet"
            );
        }
        let mut payload_end = datagram.len();
        if has_padding {
            let padding = datagram[payload_end - 1] as usize;
            ensure!(
                padding >= 1 && payload_start + padding <= payload_end,
                "invalid RTP padding length {padding}"
            );
            payload_end -= padding;
        }
        Ok((header, &datagram[payload_start..payload_end]))
    }

    pub fn encode(&self, payload: &[u8]) -> Bytes {
        let mut packet = BytesMut::with_capacity(HEADER_LENGTH + payload.len());
        packet.put_u8(VERSION << 6);
        packet.put_u8(((self.marker as u8) << 7) | (self.payload_type & 0x7F));
        packet.put_u16(self.sequence_number);
        packet.put_u32(self.timestamp);
        packet.put_u32(self.ssrc);
        packet.put_slice(payload);
        packet.freeze()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            marker: true,
            payload_type: 96,
            sequence_number: 0xBEEF,
            timestamp: 0x0102_0304,
            ssrc: 0xDEAD_BEEF,
        }
    }

    #[test]
    fn round_trips_the_fixed_header() {
        // Arrange
        let payload = [0x65, 0x88, 0x84];

        // Act
        let packet = header().encode(&payload);
        let (parsed, parsed_payload) = Header::parse(&packet).unwrap();

        // Assert
        assert_eq!(
            packet.as_ref(),
            [
                0x80, 0xE0, 0xBE, 0xEF, 1, 2, 3, 4, 0xDE, 0xAD, 0xBE, 0xEF, 0x65, 0x88, 0x84
            ]
        );
        assert_eq!(parsed, header());
        assert_eq!(parsed_payload, payload);
    }

    #[test]
    fn skips_csrc_list_header_extension_and_padding() {
        // Arrange: one CSRC, a one-word extension and two bytes of padding
        let mut packet = vec![0xB1, 0x60, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1];
        packet.extend_from_slice(&[0xCC, 0xCC, 0xCC, 0xCC]);
        packet.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x01, 0xAA, 0xAA, 0xAA, 0xAA]);
        packet.extend_from_slice(&[0x41, 0x9A, 0x00, 0x02]);

        // Act
        let (header, payload) = Header::parse(&packet).unwrap();

        // Assert
        assert!(!header.marker);
        assert_eq!(header.payload_type, 96);
        assert_eq!(payload, [0x41, 0x9A]);
    }

    #[test]
    fn rejects_other_versions_and_truncated_packets() {
        // Arrange
        let version_one = [0x40, 0x60, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1];
        let truncated_csrc = [0x81, 0x60, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1];
        let overlong_extension = [0x90, 0x60, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 9];
        let bad_padding = [0xA0, 0x60, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0x41, 0x05];

        // Act / Assert
        assert!(Header::parse(&version_one).is_err());
        assert!(Header::parse(&truncated_csrc).is_err());
        assert!(Header::parse(&overlong_extension).is_err());
        assert!(Header::parse(&bad_padding).is_err());
        assert!(Header::parse(&[0x80; 11]).is_err());
    }
}
