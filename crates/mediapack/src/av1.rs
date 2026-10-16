use std::ops::Range;

use anyhow::{Context, Result, bail, ensure};
use bytes::{BufMut, Bytes, BytesMut};

use crate::bits::BitReader;

const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_TEMPORAL_DELIMITER: u8 = 2;
const OBU_FRAME_HEADER: u8 = 3;
const OBU_FRAME: u8 = 6;
const KEY_FRAME: u64 = 0;
const CP_BT_709: u64 = 1;
const TC_SRGB: u64 = 13;
const MC_IDENTITY: u64 = 0;

/// The sequence header fields an AV1CodecConfigurationRecord carries, plus
/// the frame size and the OBU itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Av1Config {
    pub seq_profile: u8,
    pub seq_level_idx_0: u8,
    pub seq_tier_0: bool,
    pub bit_depth: u8,
    pub monochrome: bool,
    pub chroma_subsampling_x: bool,
    pub chroma_subsampling_y: bool,
    pub chroma_sample_position: u8,
    pub max_width: u32,
    pub max_height: u32,
    pub reduced_still_picture_header: bool,
    /// The sequence header OBU as the bitstream carried it, header included.
    pub sequence_header_obu: Bytes,
}

#[derive(Debug)]
pub struct Av1TemporalUnit {
    pub is_keyframe: bool,
    /// Present when the temporal unit carries a sequence header.
    pub config: Option<Av1Config>,
}

/// Reads a temporal unit in the low-overhead bitstream format (AV1
/// specification §5.2). A frame header is read with the sequence header of
/// this temporal unit or, failing that, `current`; with neither, the unit is
/// reported as an inter frame.
pub fn parse_temporal_unit(data: &Bytes, current: Option<&Av1Config>) -> Result<Av1TemporalUnit> {
    let mut config = None;
    for obu in obus(data)? {
        let payload = &data[obu.payload];
        match obu.obu_type {
            OBU_SEQUENCE_HEADER => {
                config = Some(parse_sequence_header(payload, data.slice(obu.whole))?);
            }
            OBU_FRAME_HEADER | OBU_FRAME => {
                let Some(sequence) = config.as_ref().or(current) else {
                    return Ok(Av1TemporalUnit {
                        is_keyframe: false,
                        config,
                    });
                };
                let is_keyframe = sequence.reduced_still_picture_header || {
                    let mut reader = BitReader::new(payload);
                    let show_existing_frame = reader.read_bit()?;
                    !show_existing_frame && reader.read_bits(2)? == KEY_FRAME
                };
                return Ok(Av1TemporalUnit {
                    is_keyframe,
                    config,
                });
            }
            _ => {}
        }
    }
    bail!("AV1 temporal unit without a frame")
}

/// A temporal unit as an ISOBMFF sample, which the AV1-ISOBMFF binding §2.4
/// forbids to carry temporal delimiters.
pub(crate) fn without_temporal_delimiters(data: &Bytes) -> Result<Bytes> {
    let mut sample = BytesMut::with_capacity(data.len());
    for obu in obus(data)? {
        if obu.obu_type != OBU_TEMPORAL_DELIMITER {
            sample.put_slice(&data[obu.whole]);
        }
    }
    Ok(sample.freeze())
}

/// One OBU of a temporal unit, with the byte ranges of the whole OBU and of
/// its payload.
struct Obu {
    obu_type: u8,
    whole: Range<usize>,
    payload: Range<usize>,
}

fn obus(data: &[u8]) -> Result<Vec<Obu>> {
    let mut obus = Vec::new();
    let mut position = 0;
    while position < data.len() {
        let start = position;
        let header = data[position];
        ensure!(header & 0x80 == 0, "AV1 OBU with its forbidden bit set");
        let has_extension = header & 0x04 != 0;
        let has_size_field = header & 0x02 != 0;
        position += 1 + usize::from(has_extension);
        let size = if has_size_field {
            let (size, length) = read_leb128(data.get(position..).unwrap_or_default())?;
            position += length;
            size
        } else {
            data.len().saturating_sub(position)
        };
        let end = position
            .checked_add(size)
            .filter(|end| *end <= data.len())
            .context("AV1 OBU longer than its temporal unit")?;
        obus.push(Obu {
            obu_type: (header >> 3) & 0x0f,
            whole: start..end,
            payload: position..end,
        });
        position = end;
    }
    Ok(obus)
}

/// AV1 specification §5.5.1 through `color_config()`.
fn parse_sequence_header(payload: &[u8], obu: Bytes) -> Result<Av1Config> {
    let mut reader = BitReader::new(payload);
    let seq_profile = reader.read_bits(3)? as u8;
    // still_picture
    reader.read_bit()?;
    let reduced_still_picture_header = reader.read_bit()?;
    let seq_level_idx_0;
    let mut seq_tier_0 = false;
    if reduced_still_picture_header {
        seq_level_idx_0 = reader.read_bits(5)? as u8;
    } else {
        let mut buffer_delay_length = 0;
        let mut decoder_model_info_present = false;
        if reader.read_bit()? {
            skip_timing_info(&mut reader)?;
            decoder_model_info_present = reader.read_bit()?;
            if decoder_model_info_present {
                buffer_delay_length = reader.read_bits(5)? as u32 + 1;
                // num_units_in_decoding_tick, buffer_removal_time_length_minus_1
                // and frame_presentation_time_length_minus_1
                reader.read_bits(32 + 5 + 5)?;
            }
        }
        let initial_display_delay_present = reader.read_bit()?;
        let operating_points = reader.read_bits(5)? + 1;
        let mut first = None;
        for _ in 0..operating_points {
            // operating_point_idc
            reader.read_bits(12)?;
            let seq_level_idx = reader.read_bits(5)? as u8;
            let seq_tier = seq_level_idx > 7 && reader.read_bit()?;
            first.get_or_insert((seq_level_idx, seq_tier));
            if decoder_model_info_present && reader.read_bit()? {
                // decoder_buffer_delay, encoder_buffer_delay and low_delay_mode_flag
                reader.read_bits(buffer_delay_length)?;
                reader.read_bits(buffer_delay_length + 1)?;
            }
            if initial_display_delay_present && reader.read_bit()? {
                // initial_display_delay_minus_1
                reader.read_bits(4)?;
            }
        }
        (seq_level_idx_0, seq_tier_0) = first.unwrap_or_default();
    }
    let frame_width_bits = reader.read_bits(4)? as u32 + 1;
    let frame_height_bits = reader.read_bits(4)? as u32 + 1;
    let max_width = reader.read_bits(frame_width_bits)? as u32 + 1;
    let max_height = reader.read_bits(frame_height_bits)? as u32 + 1;
    if !reduced_still_picture_header && reader.read_bit()? {
        // delta_frame_id_length_minus_2 and additional_frame_id_length_minus_1
        reader.read_bits(4 + 3)?;
    }
    // use_128x128_superblock, enable_filter_intra and enable_intra_edge_filter
    reader.read_bits(3)?;
    if !reduced_still_picture_header {
        // enable_interintra_compound, enable_masked_compound,
        // enable_warped_motion and enable_dual_filter
        reader.read_bits(4)?;
        let enable_order_hint = reader.read_bit()?;
        if enable_order_hint {
            // enable_jnt_comp and enable_ref_frame_mvs
            reader.read_bits(2)?;
        }
        let seq_force_screen_content_tools = reader.read_bit()? || reader.read_bit()?;
        if seq_force_screen_content_tools && !reader.read_bit()? {
            // seq_force_integer_mv
            reader.read_bit()?;
        }
        if enable_order_hint {
            // order_hint_bits_minus_1
            reader.read_bits(3)?;
        }
    }
    // enable_superres, enable_cdef and enable_restoration
    reader.read_bits(3)?;

    let high_bitdepth = reader.read_bit()?;
    let bit_depth = match (seq_profile, high_bitdepth) {
        (_, false) => 8,
        (2, true) if reader.read_bit()? => 12,
        (_, true) => 10,
    };
    let monochrome = seq_profile != 1 && reader.read_bit()?;
    let (color_primaries, transfer_characteristics, matrix_coefficients) = if reader.read_bit()? {
        (
            reader.read_bits(8)?,
            reader.read_bits(8)?,
            reader.read_bits(8)?,
        )
    } else {
        (2, 2, 2)
    };
    let (chroma_subsampling_x, chroma_subsampling_y) = if monochrome {
        (true, true)
    } else if color_primaries == CP_BT_709
        && transfer_characteristics == TC_SRGB
        && matrix_coefficients == MC_IDENTITY
    {
        (false, false)
    } else {
        // color_range
        reader.read_bit()?;
        match seq_profile {
            0 => (true, true),
            1 => (false, false),
            _ if bit_depth == 12 => {
                let x = reader.read_bit()?;
                (x, x && reader.read_bit()?)
            }
            _ => (true, false),
        }
    };
    let chroma_sample_position = if !monochrome && chroma_subsampling_x && chroma_subsampling_y {
        reader.read_bits(2)? as u8
    } else {
        0
    };
    Ok(Av1Config {
        seq_profile,
        seq_level_idx_0,
        seq_tier_0,
        bit_depth,
        monochrome,
        chroma_subsampling_x,
        chroma_subsampling_y,
        chroma_sample_position,
        max_width,
        max_height,
        reduced_still_picture_header,
        sequence_header_obu: obu,
    })
}

fn skip_timing_info(reader: &mut BitReader) -> Result<()> {
    // num_units_in_display_tick and time_scale
    reader.read_bits(64)?;
    // equal_picture_interval
    if reader.read_bit()? {
        // num_ticks_per_picture_minus_1 as uvlc() (§4.10.3)
        let mut leading_zeros = 0;
        while !reader.read_bit()? {
            leading_zeros += 1;
        }
        if leading_zeros < 32 {
            reader.read_bits(leading_zeros)?;
        }
    }
    Ok(())
}

/// Returns the value and the number of bytes it took (§4.10.5).
fn read_leb128(data: &[u8]) -> Result<(usize, usize)> {
    let mut value = 0_u64;
    for (index, byte) in data.iter().take(8).enumerate() {
        value |= u64::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return Ok((usize::try_from(value)?, index + 1));
        }
    }
    bail!("AV1 leb128 value truncated or longer than 8 bytes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FIXTURE_AV1_IVF, ivf_frames};

    #[test]
    fn reads_the_sequence_header_of_a_keyframe() {
        // Arrange
        let frames = ivf_frames(FIXTURE_AV1_IVF);

        // Act
        let unit = parse_temporal_unit(&frames[0], None).unwrap();

        // Assert
        assert!(unit.is_keyframe);
        let config = unit.config.unwrap();
        assert_eq!(
            (
                config.seq_profile,
                config.seq_level_idx_0,
                config.seq_tier_0,
                config.bit_depth,
                config.monochrome
            ),
            (0, 0, false, 8, false)
        );
        assert_eq!(
            (config.chroma_subsampling_x, config.chroma_subsampling_y),
            (true, true)
        );
        assert_eq!((config.max_width, config.max_height), (64, 48));
        assert_eq!(config.sequence_header_obu, frames[0].slice(2..14));
    }

    #[test]
    fn reads_an_inter_frame_with_the_current_sequence_header() {
        // Arrange
        let frames = ivf_frames(FIXTURE_AV1_IVF);
        let current = parse_temporal_unit(&frames[0], None)
            .unwrap()
            .config
            .unwrap();

        // Act
        let unit = parse_temporal_unit(&frames[1], Some(&current)).unwrap();

        // Assert
        assert!(!unit.is_keyframe);
        assert_eq!(unit.config, None);
    }

    #[test]
    fn rejects_an_obu_longer_than_its_temporal_unit() {
        // Arrange
        let frames = ivf_frames(FIXTURE_AV1_IVF);
        let truncated = frames[0].slice(..frames[0].len() - 1);

        // Act
        let error = parse_temporal_unit(&truncated, None).unwrap_err();

        // Assert
        assert_eq!(error.to_string(), "AV1 OBU longer than its temporal unit");
    }

    #[test]
    fn drops_the_temporal_delimiter_from_a_sample() {
        // Arrange: the keyframe starts with a two-byte temporal delimiter
        let frames = ivf_frames(FIXTURE_AV1_IVF);

        // Act
        let sample = without_temporal_delimiters(&frames[0]).unwrap();

        // Assert
        assert_eq!(sample, frames[0].slice(2..));
    }

    #[test]
    fn reads_a_multi_byte_leb128() {
        // Act / Assert
        assert_eq!(read_leb128(&[0x96, 0x01, 0xff]).unwrap(), (150, 2));
    }
}
