//! H.264 RTP payload format (RFC 6184): single NAL unit packets, STAP-A
//! aggregation and FU-A fragmentation, the three types packetization-mode 1
//! allows. Interleaved packetization (STAP-B, MTAP, FU-B) is rejected.

use anyhow::{Result, bail, ensure};
use bytes::{BufMut, Bytes, BytesMut};

const NAL_TYPE_MASK: u8 = 0x1F;
const NAL_HEADER_MASK: u8 = 0xE0;
const STAP_A: u8 = 24;
const STAP_B: u8 = 25;
const MTAP16: u8 = 26;
const MTAP24: u8 = 27;
const FU_A: u8 = 28;
const FU_B: u8 = 29;
const FU_START: u8 = 0x80;
const FU_END: u8 = 0x40;
const FU_HEADER_LENGTH: usize = 2;

/// Reassembles the NAL units of RTP payloads, holding an FU-A fragment across
/// packets. Call [`Depacketizer::reset`] when packets were lost so a fragment
/// cut by the loss is dropped instead of being completed by the next one.
#[derive(Default)]
pub struct Depacketizer {
    fragment: Option<BytesMut>,
}

impl Depacketizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the NAL units completed by this payload.
    pub fn push(&mut self, payload: &[u8]) -> Result<Vec<Bytes>> {
        ensure!(!payload.is_empty(), "empty H.264 RTP payload");
        match payload[0] & NAL_TYPE_MASK {
            STAP_A => self.aggregated(&payload[1..]),
            FU_A => Ok(self.fragmented(payload)?.into_iter().collect()),
            STAP_B | MTAP16 | MTAP24 | FU_B => {
                bail!("interleaved H.264 packetization is not supported")
            }
            _ => {
                self.fragment = None;
                Ok(vec![Bytes::copy_from_slice(payload)])
            }
        }
    }

    pub fn reset(&mut self) {
        self.fragment = None;
    }

    fn aggregated(&mut self, mut units: &[u8]) -> Result<Vec<Bytes>> {
        self.fragment = None;
        let mut nals = Vec::new();
        while !units.is_empty() {
            ensure!(units.len() >= 2, "STAP-A truncated in NAL unit size");
            let length = u16::from_be_bytes([units[0], units[1]]) as usize;
            ensure!(
                2 + length <= units.len(),
                "STAP-A NAL unit size exceeds the payload"
            );
            nals.push(Bytes::copy_from_slice(&units[2..2 + length]));
            units = &units[2 + length..];
        }
        Ok(nals)
    }

    fn fragmented(&mut self, payload: &[u8]) -> Result<Option<Bytes>> {
        ensure!(payload.len() > FU_HEADER_LENGTH, "FU-A carries no fragment");
        let fu_header = payload[1];
        let body = &payload[FU_HEADER_LENGTH..];
        if fu_header & FU_START != 0 {
            let nal_header = (payload[0] & NAL_HEADER_MASK) | (fu_header & NAL_TYPE_MASK);
            let mut nal = BytesMut::with_capacity(1 + body.len());
            nal.put_u8(nal_header);
            nal.put_slice(body);
            self.fragment = Some(nal);
        } else {
            // A fragment without a start belongs to a NAL unit whose start
            // was lost; it is skipped until a start fragment arrives.
            let Some(fragment) = &mut self.fragment else {
                return Ok(None);
            };
            fragment.put_slice(body);
        }
        if fu_header & FU_END == 0 {
            return Ok(None);
        }
        Ok(self.fragment.take().map(BytesMut::freeze))
    }
}

/// Splits one NAL unit into RTP payloads of at most `max_payload` bytes: the
/// unit itself when it fits, FU-A fragments otherwise. Panics when
/// `max_payload` leaves no room beyond the two FU-A header bytes.
pub fn packetize(nal: &[u8], max_payload: usize) -> Vec<Bytes> {
    assert!(
        max_payload > FU_HEADER_LENGTH,
        "an FU-A payload needs room beyond its two header bytes"
    );
    if nal.len() <= max_payload {
        return vec![Bytes::copy_from_slice(nal)];
    }
    let fu_indicator = (nal[0] & NAL_HEADER_MASK) | FU_A;
    let nal_type = nal[0] & NAL_TYPE_MASK;
    let chunks: Vec<&[u8]> = nal[1..].chunks(max_payload - FU_HEADER_LENGTH).collect();
    let last = chunks.len() - 1;
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let mut fu_header = nal_type;
            if index == 0 {
                fu_header |= FU_START;
            }
            if index == last {
                fu_header |= FU_END;
            }
            let mut payload = BytesMut::with_capacity(FU_HEADER_LENGTH + chunk.len());
            payload.put_u8(fu_indicator);
            payload.put_u8(fu_header);
            payload.put_slice(chunk);
            payload.freeze()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FIXTURE_PPS, FIXTURE_SPS, IDR_SLICE};

    fn stap_a(nals: &[&[u8]]) -> Vec<u8> {
        let mut payload = vec![0x78];
        for nal in nals {
            payload.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            payload.extend_from_slice(nal);
        }
        payload
    }

    #[test]
    fn passes_single_nal_unit_payloads_through() {
        // Arrange
        let mut depacketizer = Depacketizer::new();

        // Act
        let nals = depacketizer.push(&IDR_SLICE).unwrap();

        // Assert
        assert_eq!(nals, [Bytes::from_static(&IDR_SLICE)]);
    }

    #[test]
    fn splits_stap_a_into_its_nal_units() {
        // Arrange
        let mut depacketizer = Depacketizer::new();
        let payload = stap_a(&[&FIXTURE_SPS, &FIXTURE_PPS]);

        // Act
        let nals = depacketizer.push(&payload).unwrap();

        // Assert
        assert_eq!(
            nals,
            [
                Bytes::from_static(&FIXTURE_SPS),
                Bytes::from_static(&FIXTURE_PPS)
            ]
        );
        assert!(
            depacketizer
                .push(&stap_a(&[&[0x65][..], &[]])[..5])
                .is_err()
        );
    }

    #[test]
    fn fragments_and_reassembles_large_nal_units() {
        // Arrange
        let nal: Vec<u8> = std::iter::once(0x65)
            .chain((0..100).map(|byte| byte as u8))
            .collect();
        let mut depacketizer = Depacketizer::new();

        // Act
        let payloads = packetize(&nal, 42);
        let mut nals = Vec::new();
        for payload in &payloads {
            nals.extend(depacketizer.push(payload).unwrap());
        }

        // Assert: 100 body bytes in chunks of 40, start and end flags on the first and last
        assert_eq!(payloads.len(), 3);
        assert_eq!(&payloads[0][..2], [0x7C, 0x85]);
        assert_eq!(&payloads[1][..2], [0x7C, 0x05]);
        assert_eq!(&payloads[2][..2], [0x7C, 0x45]);
        assert_eq!(nals, [Bytes::from(nal)]);
    }

    #[test]
    fn keeps_small_nal_units_whole() {
        // Act
        let payloads = packetize(&IDR_SLICE, 1200);

        // Assert
        assert_eq!(payloads, [Bytes::from_static(&IDR_SLICE)]);
    }

    #[test]
    fn drops_a_fragment_cut_by_loss() {
        // Arrange
        let nal: Vec<u8> = std::iter::once(0x65).chain(0..60).collect();
        let payloads = packetize(&nal, 22);
        let mut depacketizer = Depacketizer::new();

        // Act: the middle fragment is lost and the reorder layer reports it
        let first = depacketizer.push(&payloads[0]).unwrap();
        depacketizer.reset();
        let last = depacketizer.push(&payloads[2]).unwrap();
        let next = depacketizer.push(&IDR_SLICE).unwrap();

        // Assert
        assert!(first.is_empty());
        assert!(last.is_empty());
        assert_eq!(next, [Bytes::from_static(&IDR_SLICE)]);
    }

    #[test]
    fn restarts_on_a_new_start_fragment() {
        // Arrange
        let first_nal: Vec<u8> = std::iter::once(0x65).chain(0..60).collect();
        let second_nal: Vec<u8> = std::iter::once(0x41).chain(100..160).collect();
        let first = packetize(&first_nal, 22);
        let second = packetize(&second_nal, 22);
        let mut depacketizer = Depacketizer::new();

        // Act: the end of the first unit never arrives
        depacketizer.push(&first[0]).unwrap();
        let mut nals = Vec::new();
        for payload in &second {
            nals.extend(depacketizer.push(payload).unwrap());
        }

        // Assert
        assert_eq!(nals, [Bytes::from(second_nal)]);
    }

    #[test]
    fn rejects_interleaved_and_empty_payloads() {
        // Arrange
        let mut depacketizer = Depacketizer::new();

        // Act / Assert
        assert!(depacketizer.push(&[]).is_err());
        assert!(depacketizer.push(&[0x79, 0, 0]).is_err());
        assert!(depacketizer.push(&[0x7D, 0x85]).is_err());
        assert!(depacketizer.push(&[0x7C]).is_err());
    }
}
