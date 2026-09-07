use anyhow::{Result, bail, ensure};

use crate::bits::BitReader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalUnitType {
    NonIdrSlice,
    SlicePartitionA,
    SlicePartitionB,
    SlicePartitionC,
    IdrSlice,
    Sei,
    Sps,
    Pps,
    AccessUnitDelimiter,
    EndOfSequence,
    EndOfStream,
    FillerData,
    Other(u8),
}

impl NalUnitType {
    pub fn from_header(header: u8) -> Self {
        match header & 0x1F {
            1 => Self::NonIdrSlice,
            2 => Self::SlicePartitionA,
            3 => Self::SlicePartitionB,
            4 => Self::SlicePartitionC,
            5 => Self::IdrSlice,
            6 => Self::Sei,
            7 => Self::Sps,
            8 => Self::Pps,
            9 => Self::AccessUnitDelimiter,
            10 => Self::EndOfSequence,
            11 => Self::EndOfStream,
            12 => Self::FillerData,
            other => Self::Other(other),
        }
    }

    pub fn is_vcl(self) -> bool {
        matches!(
            self,
            Self::NonIdrSlice
                | Self::SlicePartitionA
                | Self::SlicePartitionB
                | Self::SlicePartitionC
                | Self::IdrSlice
        )
    }

    pub fn is_parameter_set(self) -> bool {
        matches!(self, Self::Sps | Self::Pps)
    }
}

pub fn nal_unit_type(nal: &[u8]) -> Option<NalUnitType> {
    nal.first().map(|header| NalUnitType::from_header(*header))
}

pub fn starts_new_access_unit(nal: &[u8]) -> bool {
    match nal_unit_type(nal) {
        Some(
            NalUnitType::AccessUnitDelimiter
            | NalUnitType::Sps
            | NalUnitType::Pps
            | NalUnitType::Sei,
        ) => true,
        Some(NalUnitType::Other(14..=18)) => true,
        Some(kind) if kind.is_vcl() => first_mb_in_slice(nal) == Some(0),
        _ => false,
    }
}

fn first_mb_in_slice(nal: &[u8]) -> Option<u64> {
    BitReader::new(nal.get(1..)?).read_ue().ok()
}

pub fn rbsp_from_nal_payload(payload: &[u8]) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(payload.len());
    let mut zero_run = 0_usize;
    for &byte in payload {
        if zero_run >= 2 && byte == 3 {
            zero_run = 0;
            continue;
        }
        rbsp.push(byte);
        zero_run = if byte == 0 { zero_run + 1 } else { 0 };
    }
    rbsp
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceParameterSet {
    pub profile_idc: u8,
    pub constraint_flags: u8,
    pub level_idc: u8,
    pub width: u32,
    pub height: u32,
}

const HIGH_PROFILES: [u8; 13] = [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

impl SequenceParameterSet {
    pub fn parse(nal: &[u8]) -> Result<Self> {
        ensure!(
            nal_unit_type(nal) == Some(NalUnitType::Sps),
            "NAL unit is not an SPS"
        );
        let rbsp = rbsp_from_nal_payload(&nal[1..]);
        ensure!(rbsp.len() >= 4, "SPS too short: {} bytes", rbsp.len());
        let profile_idc = rbsp[0];
        let constraint_flags = rbsp[1];
        let level_idc = rbsp[2];
        let mut reader = BitReader::new(&rbsp[3..]);
        reader.read_ue()?;

        let mut chroma_format_idc = 1;
        if HIGH_PROFILES.contains(&profile_idc) {
            chroma_format_idc = reader.read_ue()?;
            if chroma_format_idc == 3 {
                reader.read_bit()?;
            }
            reader.read_ue()?;
            reader.read_ue()?;
            reader.read_bit()?;
            if reader.read_bit()? {
                let list_count = if chroma_format_idc == 3 { 12 } else { 8 };
                for index in 0..list_count {
                    if reader.read_bit()? {
                        skip_scaling_list(&mut reader, if index < 6 { 16 } else { 64 })?;
                    }
                }
            }
        }

        reader.read_ue()?;
        match reader.read_ue()? {
            0 => {
                reader.read_ue()?;
            }
            1 => {
                reader.read_bit()?;
                reader.read_se()?;
                reader.read_se()?;
                let cycle_length = reader.read_ue()?;
                for _ in 0..cycle_length {
                    reader.read_se()?;
                }
            }
            2 => {}
            other => bail!("invalid pic_order_cnt_type {other}"),
        }
        reader.read_ue()?;
        reader.read_bit()?;
        let pic_width_in_mbs = reader.read_ue()? + 1;
        let pic_height_in_map_units = reader.read_ue()? + 1;
        let frame_mbs_only = reader.read_bit()?;
        if !frame_mbs_only {
            reader.read_bit()?;
        }
        reader.read_bit()?;

        let (crop_unit_x, crop_unit_y) = match chroma_format_idc {
            0 | 3 => (1, 1),
            1 => (2, 2),
            2 => (2, 1),
            other => bail!("invalid chroma_format_idc {other}"),
        };
        let frame_height_multiplier = if frame_mbs_only { 1 } else { 2 };
        let mut width = pic_width_in_mbs * 16;
        let mut height = frame_height_multiplier * pic_height_in_map_units * 16;
        if reader.read_bit()? {
            let crop_left = reader.read_ue()?;
            let crop_right = reader.read_ue()?;
            let crop_top = reader.read_ue()?;
            let crop_bottom = reader.read_ue()?;
            width = width.saturating_sub(crop_unit_x * (crop_left + crop_right));
            height = height
                .saturating_sub(crop_unit_y * frame_height_multiplier * (crop_top + crop_bottom));
        }

        Ok(Self {
            profile_idc,
            constraint_flags,
            level_idc,
            width: width as u32,
            height: height as u32,
        })
    }

    pub fn codec_string(&self) -> String {
        format!(
            "avc1.{:02X}{:02X}{:02X}",
            self.profile_idc, self.constraint_flags, self.level_idc
        )
    }
}

fn skip_scaling_list(reader: &mut BitReader, size: usize) -> Result<()> {
    let mut last_scale = 8_i64;
    let mut next_scale = 8_i64;
    for _ in 0..size {
        if next_scale != 0 {
            let delta = reader.read_se()?;
            next_scale = (last_scale + delta + 256) % 256;
        }
        if next_scale != 0 {
            last_scale = next_scale;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FIXTURE_SPS;

    #[test]
    fn parses_baseline_sps_dimensions_and_codec() {
        // Arrange
        let sps = FIXTURE_SPS;

        // Act
        let parsed = SequenceParameterSet::parse(&sps).unwrap();

        // Assert
        assert_eq!(parsed.codec_string(), "avc1.42D00B");
        assert_eq!((parsed.width, parsed.height), (160, 90));
    }

    #[test]
    fn rejects_non_sps_nal_unit() {
        // Arrange
        let pps = [0x68, 0xCE, 0x3C, 0x80];

        // Act / Assert
        assert!(SequenceParameterSet::parse(&pps).is_err());
    }

    #[test]
    fn classifies_nal_unit_types() {
        // Arrange
        let idr = [0x65, 0x88];
        let sps = [0x67, 0x42];

        // Act / Assert
        assert_eq!(nal_unit_type(&idr), Some(NalUnitType::IdrSlice));
        assert!(NalUnitType::IdrSlice.is_vcl());
        assert_eq!(nal_unit_type(&sps), Some(NalUnitType::Sps));
        assert!(NalUnitType::Sps.is_parameter_set());
        assert_eq!(NalUnitType::from_header(0x7E), NalUnitType::Other(30));
    }

    #[test]
    fn detects_access_unit_boundaries_from_first_mb_in_slice() {
        // Arrange
        let first_slice = [0x65, 0b1000_0000];
        let later_slice = [0x41, 0b0100_0000];

        // Act / Assert
        assert!(starts_new_access_unit(&first_slice));
        assert!(!starts_new_access_unit(&later_slice));
        assert!(starts_new_access_unit(&[0x09, 0xF0]));
        assert!(!starts_new_access_unit(&[0x0C]));
    }

    #[test]
    fn removes_emulation_prevention_bytes() {
        // Arrange
        let payload = [0x00, 0x00, 0x03, 0x01, 0x00, 0x00, 0x03, 0x00];

        // Act
        let rbsp = rbsp_from_nal_payload(&payload);

        // Assert
        assert_eq!(rbsp, [0x00, 0x00, 0x01, 0x00, 0x00, 0x00]);
    }
}
