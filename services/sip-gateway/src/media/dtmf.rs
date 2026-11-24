use crate::media::g711::SAMPLES_PER_PACKET;

/// A tone lasts this many 20 ms packets (100 ms).
const TONE_PACKETS: u16 = 5;
/// The final packet is sent three times, so a lost one does not leave the
/// tone playing (RFC 4733 §2.5.1.4).
const END_PACKETS: u16 = 3;
/// The volume field: the tone's power level, -10 dBm0 (RFC 4733 §2.3.4).
const VOLUME: u8 = 10;

/// One RFC 4733 telephone-event packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventPacket {
    /// The first packet of the event (RTP marker bit, §2.5.1.2).
    pub(crate) starts: bool,
    pub(crate) payload: [u8; 4],
}

/// The packets of one digit: `TONE_PACKETS` packets whose duration grows by
/// 20 ms, then the final one, with the E bit, `END_PACKETS` times.
pub(crate) struct DtmfTone {
    event: u8,
    sent: u16,
}

impl DtmfTone {
    /// `None` for a character that is not a DTMF digit.
    pub(crate) fn new(digit: char) -> Option<Self> {
        Some(Self {
            event: event_code(digit)?,
            sent: 0,
        })
    }

    pub(crate) fn next_packet(&mut self) -> Option<EventPacket> {
        if self.sent >= TONE_PACKETS + END_PACKETS {
            return None;
        }
        let ends = self.sent >= TONE_PACKETS;
        let duration = (self.sent.min(TONE_PACKETS - 1) + 1) * SAMPLES_PER_PACKET as u16;
        let starts = self.sent == 0;
        self.sent += 1;
        let end_and_volume = if ends { 0x80 | VOLUME } else { VOLUME };
        let [high, low] = duration.to_be_bytes();
        Some(EventPacket {
            starts,
            payload: [self.event, end_and_volume, high, low],
        })
    }
}

/// The event code of a DTMF digit (RFC 4733 §3.2).
pub(crate) fn event_code(digit: char) -> Option<u8> {
    match digit {
        '0'..='9' => Some(digit as u8 - b'0'),
        '*' => Some(10),
        '#' => Some(11),
        'A'..='D' => Some(digit as u8 - b'A' + 12),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn packets(digit: char) -> Vec<EventPacket> {
        let mut tone = DtmfTone::new(digit).unwrap();
        std::iter::from_fn(|| tone.next_packet()).collect()
    }

    #[test]
    fn the_digits_take_their_rfc_4733_event_codes() {
        // Act / Assert
        assert_eq!(event_code('0'), Some(0));
        assert_eq!(event_code('9'), Some(9));
        assert_eq!(event_code('*'), Some(10));
        assert_eq!(event_code('#'), Some(11));
        assert_eq!(event_code('A'), Some(12));
        assert_eq!(event_code('D'), Some(15));
        assert_eq!(event_code('E'), None);
        assert!(DtmfTone::new('x').is_none());
    }

    #[test]
    fn a_tone_ends_with_its_final_packet_three_times() {
        // Act
        let packets = packets('5');

        // Assert
        assert_eq!(packets.len(), 8);
        assert_eq!(packets[0].payload, [5, VOLUME, 0, 160]);
        assert_eq!(packets[4].payload, [5, VOLUME, 3, 32]);
        for end in &packets[5..] {
            assert_eq!(end.payload, [5, 0x80 | VOLUME, 3, 32]);
        }
    }

    proptest! {
        /// Whatever the digit: only the first packet starts the event, the
        /// duration never shrinks, and the packets from the first with the E
        /// bit on all carry it with the tone's whole duration.
        #[test]
        fn a_tone_starts_once_grows_and_ends_with_the_e_bit(
            digit in prop::sample::select("0123456789*#ABCD".chars().collect::<Vec<_>>()),
        ) {
            let packets = packets(digit);
            let durations: Vec<u16> = packets
                .iter()
                .map(|packet| u16::from_be_bytes([packet.payload[2], packet.payload[3]]))
                .collect();
            prop_assert!(packets[0].starts);
            prop_assert!(packets[1..].iter().all(|packet| !packet.starts));
            prop_assert!(durations.windows(2).all(|pair| pair[0] <= pair[1]));
            let first_end = packets.iter().position(|packet| packet.payload[1] & 0x80 != 0).unwrap();
            prop_assert_eq!(packets.len() - first_end, END_PACKETS as usize);
            prop_assert!(packets[first_end..].iter().all(|packet| packet.payload[1] & 0x80 != 0
                && u16::from_be_bytes([packet.payload[2], packet.payload[3]]) == *durations.last().unwrap()));
        }
    }
}
