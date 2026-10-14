//! `MPEG4-GENERIC` RTP payload format for AAC (RFC 3640): an AU header
//! section followed by one or more access units, or one fragment of a large
//! access unit. Interleaving (a non-zero first AU-Index) is rejected.

use anyhow::{Result, bail, ensure};
use bytes::{BufMut, Bytes, BytesMut};

use crate::bits::{BitReader, BitWriter};

const HEADERS_LENGTH_FIELD_BITS: u32 = 16;

/// Bit widths of the AU header fields, the `sizelength`, `indexlength` and
/// `indexdeltalength` SDP parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuHeaderConfig {
    pub size_length: u8,
    pub index_length: u8,
    pub index_delta_length: u8,
}

impl AuHeaderConfig {
    /// The AAC-hbr mode (RFC 3640 §3.3.6), the one common encoders emit.
    pub const AAC_HBR: Self = Self {
        size_length: 13,
        index_length: 3,
        index_delta_length: 3,
    };

    fn header_section_bytes(self) -> usize {
        (HEADERS_LENGTH_FIELD_BITS as usize
            + self.size_length as usize
            + self.index_length as usize)
            .div_ceil(8)
    }
}

/// `frame_offset` is how many frames after the packet's RTP timestamp the
/// access unit is presented; it is 0 for the first unit of a packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessUnit {
    pub data: Bytes,
    pub frame_offset: u64,
}

/// Call [`Depacketizer::reset`] when packets were lost so a fragment cut by
/// the loss is dropped instead of being completed by the next one.
pub struct Depacketizer {
    config: AuHeaderConfig,
    fragment: Option<Fragment>,
}

struct Fragment {
    data: BytesMut,
    size: usize,
}

struct AuHeader {
    size: usize,
    frame_offset: u64,
}

impl Depacketizer {
    pub fn new(config: AuHeaderConfig) -> Self {
        Self {
            config,
            fragment: None,
        }
    }

    /// Returns the access units completed by this payload. `marker` is the
    /// RTP marker bit, which closes a fragmented access unit.
    pub fn push(&mut self, payload: &[u8], marker: bool) -> Result<Vec<AccessUnit>> {
        let (headers, mut data) = self.parse_headers(payload)?;
        if headers.len() == 1 && (data.len() < headers[0].size || self.fragment.is_some()) {
            return Ok(self
                .fragmented(&headers[0], data, marker)?
                .into_iter()
                .collect());
        }
        let mut units = Vec::with_capacity(headers.len());
        for header in headers {
            ensure!(
                header.size <= data.len(),
                "AAC access unit size {} exceeds the {} payload bytes left",
                header.size,
                data.len()
            );
            units.push(AccessUnit {
                data: Bytes::copy_from_slice(&data[..header.size]),
                frame_offset: header.frame_offset,
            });
            data = &data[header.size..];
        }
        Ok(units)
    }

    pub fn reset(&mut self) {
        self.fragment = None;
    }

    fn parse_headers<'a>(&self, payload: &'a [u8]) -> Result<(Vec<AuHeader>, &'a [u8])> {
        ensure!(
            payload.len() >= 2,
            "AAC RTP payload has no AU header section"
        );
        let header_bits = u16::from_be_bytes([payload[0], payload[1]]) as usize;
        let section_end = 2 + header_bits.div_ceil(8);
        ensure!(
            section_end <= payload.len(),
            "AU header section longer than the payload"
        );
        let mut reader = BitReader::new(&payload[2..section_end]);
        let mut headers = Vec::new();
        let mut bits_left = header_bits;
        let mut frame_offset = 0_u64;
        while bits_left > 0 {
            let index_bits = if headers.is_empty() {
                self.config.index_length
            } else {
                self.config.index_delta_length
            };
            let header_bits = self.config.size_length as usize + index_bits as usize;
            ensure!(
                bits_left >= header_bits,
                "AU header section ends inside an AU header"
            );
            let size = reader.read_bits(self.config.size_length as u32)? as usize;
            let index = reader.read_bits(index_bits as u32)?;
            if headers.is_empty() {
                ensure!(index == 0, "interleaved AAC access units are not supported");
            } else {
                frame_offset += index + 1;
            }
            headers.push(AuHeader { size, frame_offset });
            bits_left -= header_bits;
        }
        ensure!(!headers.is_empty(), "AAC RTP payload has no AU header");
        Ok((headers, &payload[section_end..]))
    }

    fn fragmented(
        &mut self,
        header: &AuHeader,
        data: &[u8],
        marker: bool,
    ) -> Result<Option<AccessUnit>> {
        if let Some(fragment) = self
            .fragment
            .take_if(|fragment| fragment.size != header.size)
        {
            bail!(
                "AAC fragment announces {} bytes while the access unit has {}",
                header.size,
                fragment.size
            );
        }
        let fragment = self.fragment.get_or_insert_with(|| Fragment {
            data: BytesMut::with_capacity(header.size),
            size: header.size,
        });
        fragment.data.put_slice(data);
        if fragment.data.len() > fragment.size {
            self.fragment = None;
            bail!("AAC fragments exceed the announced access unit size");
        }
        if !marker {
            return Ok(None);
        }
        let fragment = self.fragment.take().expect("fragment was inserted above");
        ensure!(
            fragment.data.len() == fragment.size,
            "AAC access unit ended {} bytes short",
            fragment.size - fragment.data.len()
        );
        Ok(Some(AccessUnit {
            data: fragment.data.freeze(),
            frame_offset: 0,
        }))
    }
}

/// Splits one access unit into RTP payloads of at most `max_payload` bytes.
/// Each payload carries the AU header with the full access unit size, as
/// RFC 3640 §3.2.3.1 requires for fragments.
pub fn packetize(au: &[u8], config: AuHeaderConfig, max_payload: usize) -> Result<Vec<Bytes>> {
    ensure!(
        au.len() < 1 << config.size_length,
        "AAC access unit of {} bytes does not fit a {}-bit AU-size",
        au.len(),
        config.size_length
    );
    let header_bytes = config.header_section_bytes();
    ensure!(
        max_payload > header_bytes,
        "max payload {max_payload} leaves no room after the {header_bytes}-byte AU header section"
    );
    let header_bits = config.size_length as u32 + config.index_length as u32;
    let mut header = BitWriter::new();
    header.write_bits(header_bits as u64, HEADERS_LENGTH_FIELD_BITS);
    header.write_bits(au.len() as u64, config.size_length as u32);
    header.write_bits(0, config.index_length as u32);
    let header = header.finish();
    Ok(au
        .chunks(max_payload - header_bytes)
        .map(|chunk| {
            let mut payload = BytesMut::with_capacity(header_bytes + chunk.len());
            payload.put_slice(&header);
            payload.put_slice(chunk);
            payload.freeze()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: [u8; 4] = [0x21, 0x10, 0x04, 0x60];
    const SECOND: [u8; 3] = [0xDE, 0x02, 0x00];

    fn hbr_packet(units: &[&[u8]]) -> Vec<u8> {
        let mut writer = BitWriter::new();
        writer.write_bits(16 * units.len() as u64, 16);
        for unit in units {
            writer.write_bits(unit.len() as u64, 13);
            writer.write_bits(0, 3);
        }
        let mut payload = writer.finish();
        for unit in units {
            payload.extend_from_slice(unit);
        }
        payload
    }

    #[test]
    fn round_trips_a_single_access_unit() {
        // Arrange
        let mut depacketizer = Depacketizer::new(AuHeaderConfig::AAC_HBR);

        // Act
        let payloads = packetize(&FIRST, AuHeaderConfig::AAC_HBR, 1200).unwrap();
        let units = depacketizer.push(&payloads[0], true).unwrap();

        // Assert: 16-bit header length, 13-bit size 4 and a 3-bit index 0
        assert_eq!(payloads.len(), 1);
        assert_eq!(
            payloads[0].as_ref(),
            [0x00, 0x10, 0x00, 0x20, 0x21, 0x10, 0x04, 0x60]
        );
        assert_eq!(
            units,
            [AccessUnit {
                data: Bytes::from_static(&FIRST),
                frame_offset: 0
            }]
        );
    }

    #[test]
    fn splits_a_packet_holding_several_access_units() {
        // Arrange
        let mut depacketizer = Depacketizer::new(AuHeaderConfig::AAC_HBR);

        // Act
        let units = depacketizer
            .push(&hbr_packet(&[&FIRST, &SECOND]), true)
            .unwrap();

        // Assert: consecutive units are one frame apart
        assert_eq!(
            units,
            [
                AccessUnit {
                    data: Bytes::from_static(&FIRST),
                    frame_offset: 0
                },
                AccessUnit {
                    data: Bytes::from_static(&SECOND),
                    frame_offset: 1
                }
            ]
        );
    }

    #[test]
    fn fragments_and_reassembles_a_large_access_unit() {
        // Arrange
        let au: Vec<u8> = (0..100).map(|byte| byte as u8).collect();
        let mut depacketizer = Depacketizer::new(AuHeaderConfig::AAC_HBR);

        // Act
        let payloads = packetize(&au, AuHeaderConfig::AAC_HBR, 44).unwrap();
        let last = payloads.len() - 1;
        let mut units = Vec::new();
        for (index, payload) in payloads.iter().enumerate() {
            units.extend(depacketizer.push(payload, index == last).unwrap());
        }

        // Assert: 4 header bytes leave 40 bytes per fragment
        assert_eq!(payloads.len(), 3);
        assert!(
            payloads
                .iter()
                .all(|payload| payload[..4] == [0, 0x10, 0x03, 0x20])
        );
        assert_eq!(
            units,
            [AccessUnit {
                data: Bytes::from(au),
                frame_offset: 0
            }]
        );
    }

    #[test]
    fn drops_a_fragment_cut_by_loss() {
        // Arrange
        let au: Vec<u8> = (0..100).map(|byte| byte as u8).collect();
        let payloads = packetize(&au, AuHeaderConfig::AAC_HBR, 44).unwrap();
        let mut depacketizer = Depacketizer::new(AuHeaderConfig::AAC_HBR);

        // Act: the middle fragment is lost
        depacketizer.push(&payloads[0], false).unwrap();
        depacketizer.reset();
        let short = depacketizer.push(&payloads[2], true);
        let next = depacketizer.push(&hbr_packet(&[&FIRST]), true).unwrap();

        // Assert
        assert!(short.is_err());
        assert_eq!(next[0].data, Bytes::from_static(&FIRST));
    }

    #[test]
    fn rejects_malformed_payloads() {
        // Arrange
        let mut depacketizer = Depacketizer::new(AuHeaderConfig::AAC_HBR);
        let interleaved = [0x00, 0x10, 0x00, 0x21, 0x21];
        let truncated_data = [0x00, 0x10, 0x00, 0x40, 0x21];
        let overlong_section = [0x00, 0x30, 0x00, 0x20];

        // Act / Assert
        assert!(depacketizer.push(&[0x00], true).is_err());
        assert!(depacketizer.push(&interleaved, true).is_err());
        assert!(depacketizer.push(&truncated_data, true).is_err());
        assert!(depacketizer.push(&overlong_section, true).is_err());
        assert!(packetize(&[0; 8192], AuHeaderConfig::AAC_HBR, 1200).is_err());
        assert!(packetize(&FIRST, AuHeaderConfig::AAC_HBR, 4).is_err());
    }
}
