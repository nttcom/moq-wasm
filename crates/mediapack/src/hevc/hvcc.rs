use anyhow::{Result, ensure};
use bytes::{BufMut, Bytes, BytesMut};

use crate::{
    hevc::nal::{
        ACCESS_UNIT_DELIMITER, GENERAL_PROFILE_TIER_LEVEL_LENGTH, IRAP_SLICES, PPS, SPS,
        SequenceParameterSet, VPS, nal_unit_type,
    },
    parameter_sets::{NalDecoderConfig, NalKind},
    sample::VideoConfig,
};

const CONFIGURATION_VERSION: u8 = 1;
/// configurationVersion to lengthSizeMinusOne; numOfArrays follows.
const FIXED_LENGTH: usize = 22;

/// HEVCDecoderConfigurationRecord (ISO/IEC 14496-15 §8.3.3.1.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HevcDecoderConfigurationRecord {
    /// general_profile_space to general_level_idc, as the SPS holds them.
    pub general_profile_tier_level: [u8; GENERAL_PROFILE_TIER_LEVEL_LENGTH],
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub temporal_layers: u8,
    pub temporal_id_nested: bool,
    pub nal_length_size: u8,
    pub vps: Vec<Bytes>,
    pub sps: Vec<Bytes>,
    pub pps: Vec<Bytes>,
}

impl HevcDecoderConfigurationRecord {
    /// Keeps the VPS, SPS and PPS arrays and skips any other, such as SEI.
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(
            data.len() > FIXED_LENGTH,
            "HEVC decoder configuration record too short"
        );
        ensure!(
            data[0] == CONFIGURATION_VERSION,
            "unsupported HEVC configuration version {}",
            data[0]
        );
        let mut general_profile_tier_level = [0; GENERAL_PROFILE_TIER_LEVEL_LENGTH];
        general_profile_tier_level.copy_from_slice(&data[1..1 + GENERAL_PROFILE_TIER_LEVEL_LENGTH]);
        let mut record = Self {
            general_profile_tier_level,
            chroma_format_idc: data[16] & 0b11,
            bit_depth_luma: (data[17] & 0b111) + 8,
            bit_depth_chroma: (data[18] & 0b111) + 8,
            temporal_layers: (data[21] >> 3) & 0b111,
            temporal_id_nested: data[21] & 0b100 != 0,
            nal_length_size: (data[21] & 0b11) + 1,
            vps: Vec::new(),
            sps: Vec::new(),
            pps: Vec::new(),
        };
        let mut offset = FIXED_LENGTH + 1;
        for _ in 0..data[FIXED_LENGTH] {
            let header = read(data, &mut offset, 3)?;
            let count = u16::from_be_bytes([header[1], header[2]]);
            let mut sets = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                let length = read(data, &mut offset, 2)?;
                let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
                sets.push(Bytes::copy_from_slice(read(data, &mut offset, length)?));
            }
            match header[0] & 0x3F {
                VPS => record.vps = sets,
                SPS => record.sps = sets,
                PPS => record.pps = sets,
                _ => {}
            }
        }
        Ok(record)
    }

    pub fn from_parameter_sets(vps: Vec<Bytes>, sps: Vec<Bytes>, pps: Vec<Bytes>) -> Result<Self> {
        let first_sps = sps
            .first()
            .ok_or_else(|| anyhow::anyhow!("no SPS available"))?;
        let parsed = SequenceParameterSet::parse(first_sps)?;
        Ok(Self {
            general_profile_tier_level: parsed.general_profile_tier_level,
            chroma_format_idc: parsed.chroma_format_idc,
            bit_depth_luma: parsed.bit_depth_luma,
            bit_depth_chroma: parsed.bit_depth_chroma,
            temporal_layers: parsed.temporal_layers,
            temporal_id_nested: parsed.temporal_id_nested,
            nal_length_size: 4,
            vps,
            sps,
            pps,
        })
    }

    /// Leaves min_spatial_segmentation_idc, parallelismType, avgFrameRate and
    /// constantFrameRate at 0, which means unknown, and marks every array
    /// complete, as the `hvc1` sample entry requires.
    pub fn to_bytes(&self) -> Bytes {
        let mut out = BytesMut::new();
        out.put_u8(CONFIGURATION_VERSION);
        out.put_slice(&self.general_profile_tier_level);
        out.put_u16(0xF000);
        out.put_u8(0xFC);
        out.put_u8(0xFC | self.chroma_format_idc);
        out.put_u8(0xF8 | (self.bit_depth_luma - 8));
        out.put_u8(0xF8 | (self.bit_depth_chroma - 8));
        out.put_u16(0);
        out.put_u8(
            self.temporal_layers << 3
                | u8::from(self.temporal_id_nested) << 2
                | (self.nal_length_size - 1),
        );
        let arrays = [(VPS, &self.vps), (SPS, &self.sps), (PPS, &self.pps)];
        out.put_u8(arrays.len() as u8);
        for (nal_unit_type, sets) in arrays {
            out.put_u8(0x80 | nal_unit_type);
            out.put_u16(sets.len() as u16);
            for set in sets {
                out.put_u16(set.len() as u16);
                out.put_slice(set);
            }
        }
        out.freeze()
    }

    /// The `hvc1` codec string of ISO/IEC 14496-15 Annex E.3.
    pub fn codec_string(&self) -> String {
        let profile = self.general_profile_tier_level[0];
        let profile_space = ["", "A", "B", "C"][usize::from(profile >> 6)];
        let tier = if profile & 0x20 == 0 { 'L' } else { 'H' };
        let [_, c0, c1, c2, c3, constraints @ .., level] = self.general_profile_tier_level;
        // The string reads the compatibility flags from flag 31 down to flag 0.
        let compatibility = u32::from_be_bytes([c0, c1, c2, c3]).reverse_bits();
        let mut codec = format!(
            "hvc1.{profile_space}{}.{compatibility:X}.{tier}{level}",
            profile & 0x1F
        );
        let used = constraints
            .iter()
            .rposition(|byte| *byte != 0)
            .map_or(0, |last| last + 1);
        for byte in &constraints[..used] {
            codec.push_str(&format!(".{byte:X}"));
        }
        codec
    }

    pub fn sequence_parameter_set(&self) -> Result<SequenceParameterSet> {
        let sps = self
            .sps
            .first()
            .ok_or_else(|| anyhow::anyhow!("no SPS in HEVC configuration record"))?;
        SequenceParameterSet::parse(sps)
    }
}

fn read<'a>(data: &'a [u8], offset: &mut usize, length: usize) -> Result<&'a [u8]> {
    let bytes = data
        .get(*offset..*offset + length)
        .ok_or_else(|| anyhow::anyhow!("HEVC configuration record truncated"))?;
    *offset += length;
    Ok(bytes)
}

impl NalDecoderConfig for HevcDecoderConfigurationRecord {
    const PARAMETER_SET_KINDS: usize = 3;
    const SEQUENCE_PARAMETER_SET: usize = 1;

    fn nal_kind(nal: &[u8]) -> NalKind {
        match nal_unit_type(nal) {
            Some(nal_unit_type) if IRAP_SLICES.contains(&nal_unit_type) => NalKind::KeyframeSlice,
            Some(VPS) => NalKind::ParameterSet(0),
            Some(SPS) => NalKind::ParameterSet(1),
            Some(PPS) => NalKind::ParameterSet(2),
            Some(ACCESS_UNIT_DELIMITER) => NalKind::AccessUnitDelimiter,
            _ => NalKind::Other,
        }
    }

    fn from_parameter_sets(sets: Vec<Vec<Bytes>>) -> Result<Self> {
        let [vps, sps, pps]: [Vec<Bytes>; 3] = sets
            .try_into()
            .map_err(|_| anyhow::anyhow!("H.265 takes VPS, SPS and PPS"))?;
        Self::from_parameter_sets(vps, sps, pps)
    }

    fn parameter_sets(&self) -> Vec<&[Bytes]> {
        vec![&self.vps, &self.sps, &self.pps]
    }

    fn parse(data: &[u8]) -> Result<Self> {
        Self::parse(data)
    }

    fn nal_length_size(&self) -> u8 {
        self.nal_length_size
    }
}

impl From<HevcDecoderConfigurationRecord> for VideoConfig {
    fn from(record: HevcDecoderConfigurationRecord) -> Self {
        Self::Hevc(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        parameter_sets::ParameterSetTracker,
        test_support::{
            FIXTURE_HVCC, hevc_delta_frame_annexb, hevc_fixture_nal, hevc_fixture_record,
            hevc_keyframe_annexb,
        },
    };

    #[test]
    fn writes_the_record_ffmpeg_writes_for_the_same_parameter_sets() {
        // Arrange
        let record = hevc_fixture_record();

        // Act
        let bytes = record.to_bytes();

        // Assert
        assert_eq!(bytes.as_ref(), FIXTURE_HVCC);
    }

    #[test]
    fn a_parsed_record_keeps_what_it_was_written_from() {
        // Arrange
        let record = hevc_fixture_record();

        // Act
        let parsed = HevcDecoderConfigurationRecord::parse(&record.to_bytes()).unwrap();

        // Assert
        assert_eq!(parsed, record);
    }

    #[test]
    fn rejects_a_truncated_record() {
        // Arrange
        let bytes = hevc_fixture_record().to_bytes();

        // Act / Assert
        assert!(HevcDecoderConfigurationRecord::parse(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn names_the_profile_tier_and_level_in_the_codec_string() {
        // Arrange
        let main = hevc_fixture_record();
        let mut main10_high_tier = main.clone();
        main10_high_tier.general_profile_tier_level =
            [0x22, 0x20, 0, 0, 0, 0xB0, 0, 0, 0x08, 0, 0, 153];

        // Act / Assert: trailing zero constraint bytes are left out
        assert_eq!(main.codec_string(), "hvc1.1.6.L30.90");
        assert_eq!(main10_high_tier.codec_string(), "hvc1.2.4.H153.B0.0.0.8");
    }

    #[test]
    fn a_keyframe_reports_the_record_and_a_bare_one_gets_its_parameter_sets() {
        // Arrange
        let mut tracker = ParameterSetTracker::<HevcDecoderConfigurationRecord>::new();
        let bare_keyframe = crate::h264::annexb::with_start_codes([&hevc_fixture_nal(20)[..]]);

        // Act
        let first = tracker.track(&hevc_keyframe_annexb()).unwrap().unwrap();
        let delta = tracker.track(&hevc_delta_frame_annexb()).unwrap().unwrap();
        let completed = tracker.track(&bare_keyframe).unwrap().unwrap();

        // Assert
        assert_eq!(first.config_changed, Some(hevc_fixture_record()));
        assert!(first.is_keyframe);
        assert_eq!(first.data, hevc_keyframe_annexb());
        assert!(!delta.is_keyframe);
        assert_eq!(delta.data, hevc_delta_frame_annexb());
        assert!(completed.config_changed.is_none());
        assert_eq!(completed.data, hevc_keyframe_annexb());
    }
}
