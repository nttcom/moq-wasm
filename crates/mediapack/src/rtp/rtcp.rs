use bytes::{BufMut, Bytes, BytesMut};

use crate::sample::Timestamp;

const VERSION: u8 = 2;
const SENDER_REPORT: u8 = 200;
const SOURCE_DESCRIPTION: u8 = 202;
const CNAME_ITEM: u8 = 1;
const SENDER_REPORT_WORDS: u16 = 6;
/// Seconds from the NTP epoch (1900) to the Unix epoch (1970).
const NTP_UNIX_OFFSET_SECONDS: u64 = 2_208_988_800;
const MICROS_PER_SECOND: u64 = 1_000_000;

/// An RTCP sender report (RFC 3550 §6.4.1) for one stream. `wallclock` and
/// `rtp_timestamp` name the same instant, which is what lets a receiver
/// align streams that share `cname`.
pub(crate) struct SenderReport<'a> {
    pub(crate) ssrc: u32,
    /// Microseconds since the Unix epoch.
    pub(crate) wallclock: Timestamp,
    pub(crate) rtp_timestamp: u32,
    pub(crate) packet_count: u32,
    pub(crate) octet_count: u32,
    pub(crate) cname: &'a str,
}

impl SenderReport<'_> {
    /// The report and an SDES packet with its CNAME in one compound packet,
    /// the minimum RFC 3550 §6.1 allows.
    pub(crate) fn encode(&self) -> Bytes {
        let mut packet = BytesMut::new();
        packet.put_u8(VERSION << 6);
        packet.put_u8(SENDER_REPORT);
        packet.put_u16(SENDER_REPORT_WORDS);
        packet.put_u32(self.ssrc);
        packet.put_u64(ntp_timestamp(self.wallclock));
        packet.put_u32(self.rtp_timestamp);
        packet.put_u32(self.packet_count);
        packet.put_u32(self.octet_count);

        // An SDES item holds at most 255 octets; the item list ends with at
        // least one null octet and pads the chunk to a 32-bit boundary.
        let cname = &self.cname.as_bytes()[..self.cname.len().min(255)];
        let items = 2 + cname.len();
        let chunk = 4 + (items + 1).div_ceil(4) * 4;
        packet.put_u8((VERSION << 6) | 1);
        packet.put_u8(SOURCE_DESCRIPTION);
        packet.put_u16((chunk / 4) as u16);
        packet.put_u32(self.ssrc);
        packet.put_u8(CNAME_ITEM);
        packet.put_u8(cname.len() as u8);
        packet.put_slice(cname);
        packet.put_bytes(0, chunk - 4 - items);
        packet.freeze()
    }
}

/// The 64-bit NTP format: seconds since 1900, then the fraction of a second
/// in units of 2^-32 s.
fn ntp_timestamp(wallclock: Timestamp) -> u64 {
    let micros = wallclock.micros();
    let seconds = micros / MICROS_PER_SECOND + NTP_UNIX_OFFSET_SECONDS;
    let fraction = ((micros % MICROS_PER_SECOND) << 32) / MICROS_PER_SECOND;
    (seconds << 32) | fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_sender_report_with_its_cname() {
        // Arrange: 2026-10-14T00:00:00.5Z
        let report = SenderReport {
            ssrc: 0x0102_0304,
            wallclock: Timestamp::from_micros(1_791_936_000_500_000),
            rtp_timestamp: 0xAABB_CCDD,
            packet_count: 7,
            octet_count: 9_000,
            cname: "abc",
        };

        // Act
        let packet = report.encode();

        // Assert: 28 bytes of report, then a 16-byte SDES packet padded after "abc"
        assert_eq!(packet.len(), 28 + 16);
        assert_eq!(&packet[..4], [0x80, 200, 0, 6]);
        assert_eq!(&packet[4..8], [1, 2, 3, 4]);
        assert_eq!(
            u32::from_be_bytes(packet[8..12].try_into().unwrap()) as u64,
            1_791_936_000 + NTP_UNIX_OFFSET_SECONDS
        );
        assert_eq!(&packet[12..16], [0x80, 0, 0, 0]);
        assert_eq!(
            &packet[16..28],
            [0xAA, 0xBB, 0xCC, 0xDD, 0, 0, 0, 7, 0, 0, 0x23, 0x28]
        );
        assert_eq!(
            &packet[28..],
            [0x81, 202, 0, 3, 1, 2, 3, 4, 1, 3, b'a', b'b', b'c', 0, 0, 0]
        );
    }

    #[test]
    fn terminates_an_item_list_that_ends_on_a_word_boundary() {
        // Arrange: CNAME item of 2 + 2 octets fills the word, so a whole null word follows
        let report = SenderReport {
            ssrc: 1,
            wallclock: Timestamp::ZERO,
            rtp_timestamp: 0,
            packet_count: 0,
            octet_count: 0,
            cname: "ab",
        };

        // Act
        let packet = report.encode();

        // Assert
        assert_eq!(
            &packet[28..],
            [0x81, 202, 0, 3, 0, 0, 0, 1, 1, 2, b'a', b'b', 0, 0, 0, 0]
        );
    }
}
