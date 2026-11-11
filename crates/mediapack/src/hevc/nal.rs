use std::ops::RangeInclusive;

use anyhow::{Result, bail, ensure};

use crate::{bits::BitReader, h264::nal::rbsp_from_nal_payload};

// ITU-T H.265 Table 7-1.
pub(crate) const VPS: u8 = 32;
pub(crate) const SPS: u8 = 33;
pub(crate) const PPS: u8 = 34;
pub(crate) const ACCESS_UNIT_DELIMITER: u8 = 35;
/// The slices of an IRAP picture: BLA, IDR and CRA.
pub(crate) const IRAP_SLICES: RangeInclusive<u8> = 16..=23;

const NAL_HEADER_LENGTH: usize = 2;
/// general_profile_space to general_level_idc of profile_tier_level()
/// (ITU-T H.265 §7.3.3).
pub(crate) const GENERAL_PROFILE_TIER_LEVEL_LENGTH: usize = 12;
/// A sub-layer's profile_space to its constraint flags, and its level_idc.
const SUB_LAYER_PROFILE_BITS: u32 = 88;
const SUB_LAYER_LEVEL_BITS: u32 = 8;

pub(crate) fn nal_unit_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|header| (header >> 1) & 0x3F)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceParameterSet {
    /// general_profile_space, general_tier_flag, general_profile_idc, the
    /// profile compatibility flags, the constraint indicator flags and
    /// general_level_idc, as the bytes the SPS holds them in.
    pub general_profile_tier_level: [u8; GENERAL_PROFILE_TIER_LEVEL_LENGTH],
    pub temporal_layers: u8,
    pub temporal_id_nested: bool,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub width: u32,
    pub height: u32,
}

impl SequenceParameterSet {
    /// Reads seq_parameter_set_rbsp() (ITU-T H.265 §7.3.2.2) up to the bit
    /// depths.
    pub fn parse(nal: &[u8]) -> Result<Self> {
        ensure!(nal_unit_type(nal) == Some(SPS), "NAL unit is not an SPS");
        ensure!(nal.len() > NAL_HEADER_LENGTH, "SPS too short");
        let rbsp = rbsp_from_nal_payload(&nal[NAL_HEADER_LENGTH..]);
        ensure!(
            rbsp.len() > GENERAL_PROFILE_TIER_LEVEL_LENGTH,
            "SPS too short: {} bytes",
            rbsp.len()
        );
        // sps_video_parameter_set_id takes the first 4 bits.
        let max_sub_layers_minus1 = (rbsp[0] >> 1) & 0b111;
        let temporal_id_nested = rbsp[0] & 1 == 1;
        let mut general_profile_tier_level = [0; GENERAL_PROFILE_TIER_LEVEL_LENGTH];
        general_profile_tier_level.copy_from_slice(&rbsp[1..1 + GENERAL_PROFILE_TIER_LEVEL_LENGTH]);

        let mut reader = BitReader::new(&rbsp[1 + GENERAL_PROFILE_TIER_LEVEL_LENGTH..]);
        let mut sub_layer_bits = 0;
        for _ in 0..max_sub_layers_minus1 {
            if reader.read_bit()? {
                sub_layer_bits += SUB_LAYER_PROFILE_BITS;
            }
            if reader.read_bit()? {
                sub_layer_bits += SUB_LAYER_LEVEL_BITS;
            }
        }
        if max_sub_layers_minus1 > 0 {
            // reserved_zero_2bits for the sub-layers up to the eighth.
            reader.read_bits(2 * (8 - u32::from(max_sub_layers_minus1)))?;
        }
        for _ in 0..sub_layer_bits / 8 {
            reader.read_bits(8)?;
        }

        reader.read_ue()?;
        let chroma_format_idc = reader.read_ue()?;
        if chroma_format_idc == 3 {
            reader.read_bit()?;
        }
        let mut width = reader.read_ue()?;
        let mut height = reader.read_ue()?;
        if reader.read_bit()? {
            let (crop_unit_x, crop_unit_y) = match chroma_format_idc {
                0 | 3 => (1, 1),
                1 => (2, 2),
                2 => (2, 1),
                other => bail!("invalid chroma_format_idc {other}"),
            };
            let left = reader.read_ue()?;
            let right = reader.read_ue()?;
            let top = reader.read_ue()?;
            let bottom = reader.read_ue()?;
            width = width.saturating_sub(crop_unit_x * (left + right));
            height = height.saturating_sub(crop_unit_y * (top + bottom));
        }
        let bit_depth_luma = reader.read_ue()? + 8;
        let bit_depth_chroma = reader.read_ue()? + 8;

        Ok(Self {
            general_profile_tier_level,
            temporal_layers: max_sub_layers_minus1 + 1,
            temporal_id_nested,
            chroma_format_idc: u8::try_from(chroma_format_idc)?,
            bit_depth_luma: u8::try_from(bit_depth_luma)?,
            bit_depth_chroma: u8::try_from(bit_depth_chroma)?,
            width: u32::try_from(width)?,
            height: u32::try_from(height)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::hevc_fixture_nal;

    #[test]
    fn reads_the_profile_and_size_of_an_sps() {
        // Arrange
        let sps = hevc_fixture_nal(SPS);

        // Act
        let parsed = SequenceParameterSet::parse(&sps).unwrap();

        // Assert: Main profile, level 1 (30), 64x48 4:2:0 at 8 bits
        assert_eq!(
            parsed,
            SequenceParameterSet {
                general_profile_tier_level: [1, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 30],
                temporal_layers: 1,
                temporal_id_nested: true,
                chroma_format_idc: 1,
                bit_depth_luma: 8,
                bit_depth_chroma: 8,
                width: 64,
                height: 48,
            }
        );
    }

    #[test]
    fn rejects_a_nal_unit_that_is_not_an_sps() {
        // Arrange
        let pps = hevc_fixture_nal(PPS);

        // Act / Assert
        assert!(SequenceParameterSet::parse(&pps).is_err());
    }
}
