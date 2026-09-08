use anyhow::{Result, ensure};
use bytes::{BufMut, Bytes, BytesMut};

use crate::h264::{annexb::with_start_codes, nal::SequenceParameterSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvcDecoderConfigurationRecord {
    pub profile_idc: u8,
    pub profile_compatibility: u8,
    pub level_idc: u8,
    pub nal_length_size: u8,
    pub sps: Vec<Bytes>,
    pub pps: Vec<Bytes>,
}

impl AvcDecoderConfigurationRecord {
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(
            data.len() >= 7,
            "AVC decoder configuration record too short"
        );
        ensure!(
            data[0] == 1,
            "unsupported AVC configuration version {}",
            data[0]
        );
        let nal_length_size = (data[4] & 0b11) + 1;
        let mut offset = 6;
        let sps = read_parameter_sets(data, &mut offset, (data[5] & 0x1F) as usize)?;
        ensure!(
            offset < data.len(),
            "AVC configuration record missing PPS count"
        );
        let pps_count = data[offset] as usize;
        offset += 1;
        let pps = read_parameter_sets(data, &mut offset, pps_count)?;
        Ok(Self {
            profile_idc: data[1],
            profile_compatibility: data[2],
            level_idc: data[3],
            nal_length_size,
            sps,
            pps,
        })
    }

    pub fn from_parameter_sets(sps: Vec<Bytes>, pps: Vec<Bytes>) -> Result<Self> {
        let first_sps = sps
            .first()
            .ok_or_else(|| anyhow::anyhow!("no SPS available"))?;
        let parsed = SequenceParameterSet::parse(first_sps)?;
        Ok(Self {
            profile_idc: parsed.profile_idc,
            profile_compatibility: parsed.constraint_flags,
            level_idc: parsed.level_idc,
            nal_length_size: 4,
            sps,
            pps,
        })
    }

    pub fn to_bytes(&self) -> Bytes {
        let mut out = BytesMut::new();
        out.put_u8(1);
        out.put_u8(self.profile_idc);
        out.put_u8(self.profile_compatibility);
        out.put_u8(self.level_idc);
        out.put_u8(0xFC | (self.nal_length_size - 1));
        out.put_u8(0xE0 | self.sps.len() as u8);
        for sps in &self.sps {
            out.put_u16(sps.len() as u16);
            out.put_slice(sps);
        }
        out.put_u8(self.pps.len() as u8);
        for pps in &self.pps {
            out.put_u16(pps.len() as u16);
            out.put_slice(pps);
        }
        out.freeze()
    }

    pub fn codec_string(&self) -> String {
        format!(
            "avc1.{:02X}{:02X}{:02X}",
            self.profile_idc, self.profile_compatibility, self.level_idc
        )
    }

    pub fn sequence_parameter_set(&self) -> Result<SequenceParameterSet> {
        let sps = self
            .sps
            .first()
            .ok_or_else(|| anyhow::anyhow!("no SPS in AVC configuration record"))?;
        SequenceParameterSet::parse(sps)
    }

    pub fn parameter_sets_annexb(&self) -> Bytes {
        with_start_codes(
            self.sps
                .iter()
                .chain(self.pps.iter())
                .map(|set| set.as_ref()),
        )
    }
}

fn read_parameter_sets(data: &[u8], offset: &mut usize, count: usize) -> Result<Vec<Bytes>> {
    let mut sets = Vec::with_capacity(count);
    for _ in 0..count {
        ensure!(
            *offset + 2 <= data.len(),
            "AVC configuration record truncated in parameter set length"
        );
        let length = u16::from_be_bytes([data[*offset], data[*offset + 1]]) as usize;
        *offset += 2;
        ensure!(
            *offset + length <= data.len(),
            "AVC configuration record truncated in parameter set body"
        );
        sets.push(Bytes::copy_from_slice(&data[*offset..*offset + length]));
        *offset += length;
    }
    Ok(sets)
}

pub fn length_prefixed<'a>(
    nals: impl IntoIterator<Item = &'a [u8]>,
    nal_length_size: usize,
) -> Bytes {
    let mut out = BytesMut::new();
    for nal in nals {
        let length = (nal.len() as u32).to_be_bytes();
        out.put_slice(&length[4 - nal_length_size..]);
        out.put_slice(nal);
    }
    out.freeze()
}

pub fn avcc_to_annexb(data: &[u8], nal_length_size: usize) -> Result<Bytes> {
    ensure!(
        (1..=4).contains(&nal_length_size),
        "invalid NAL length size {nal_length_size}"
    );
    let mut nals = Vec::new();
    let mut offset = 0;
    while offset + nal_length_size <= data.len() {
        let mut length_bytes = [0_u8; 4];
        length_bytes[4 - nal_length_size..]
            .copy_from_slice(&data[offset..offset + nal_length_size]);
        offset += nal_length_size;
        let length = u32::from_be_bytes(length_bytes) as usize;
        ensure!(
            offset + length <= data.len(),
            "AVCC NAL unit length {length} exceeds remaining {} bytes",
            data.len() - offset
        );
        nals.push(&data[offset..offset + length]);
        offset += length;
    }
    ensure!(
        offset == data.len(),
        "trailing bytes after last AVCC NAL unit"
    );
    Ok(with_start_codes(nals))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_bytes() -> Vec<u8> {
        vec![
            1, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0, 3, 0x67, 0x42, 0xC0, 1, 0, 2, 0x68, 0xCE,
        ]
    }

    #[test]
    fn parses_and_serializes_configuration_record() {
        // Arrange
        let bytes = record_bytes();

        // Act
        let record = AvcDecoderConfigurationRecord::parse(&bytes).unwrap();

        // Assert
        assert_eq!(record.codec_string(), "avc1.42C01E");
        assert_eq!(record.nal_length_size, 4);
        assert_eq!(record.sps, [Bytes::from_static(&[0x67, 0x42, 0xC0])]);
        assert_eq!(record.pps, [Bytes::from_static(&[0x68, 0xCE])]);
        assert_eq!(record.to_bytes(), bytes);
    }

    #[test]
    fn rejects_truncated_record() {
        // Arrange
        let mut bytes = record_bytes();
        bytes.truncate(10);

        // Act
        let result = AvcDecoderConfigurationRecord::parse(&bytes);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn converts_avcc_to_annexb() {
        // Arrange
        let avcc = [0, 0, 0, 2, 0x65, 0xCC, 0, 0, 0, 1, 0x41];

        // Act
        let annexb = avcc_to_annexb(&avcc, 4).unwrap();

        // Assert
        assert_eq!(annexb.as_ref(), [0, 0, 0, 1, 0x65, 0xCC, 0, 0, 0, 1, 0x41]);
    }

    #[test]
    fn rejects_avcc_with_overlong_nal_unit() {
        // Arrange
        let avcc = [0, 0, 0, 9, 0x65];

        // Act / Assert
        assert!(avcc_to_annexb(&avcc, 4).is_err());
    }

    #[test]
    fn emits_parameter_sets_as_annexb() {
        // Arrange
        let record = AvcDecoderConfigurationRecord::parse(&record_bytes()).unwrap();

        // Act
        let annexb = record.parameter_sets_annexb();

        // Assert
        assert_eq!(
            annexb.as_ref(),
            [0, 0, 0, 1, 0x67, 0x42, 0xC0, 0, 0, 0, 1, 0x68, 0xCE]
        );
    }
}
