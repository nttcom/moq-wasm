use anyhow::{Result, ensure};

use crate::bits::BitReader;

const FRAME_MARKER: u64 = 2;
const SYNC_CODE: u64 = 0x49_83_42;
const CS_RGB: u64 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vp9Config {
    pub profile: u8,
    pub bit_depth: u8,
    pub subsampling_x: bool,
    pub subsampling_y: bool,
    pub full_range: bool,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub struct Vp9Frame {
    pub is_keyframe: bool,
    /// Present on keyframes only.
    pub config: Option<Vp9Config>,
}

/// Reads the start of the uncompressed header (VP9 bitstream specification
/// §6.2). In a superframe the first frame is the one read, which is the
/// keyframe when the superframe holds one.
pub fn parse_frame(data: &[u8]) -> Result<Vp9Frame> {
    let mut reader = BitReader::new(data);
    ensure!(
        reader.read_bits(2)? == FRAME_MARKER,
        "VP9 frame without its frame marker"
    );
    let profile_low = reader.read_bits(1)?;
    let profile = ((reader.read_bits(1)? << 1) | profile_low) as u8;
    if profile == 3 {
        reader.read_bits(1)?;
    }
    let show_existing_frame = reader.read_bit()?;
    // frame_type 0 is KEY_FRAME.
    if show_existing_frame || reader.read_bit()? {
        return Ok(Vp9Frame {
            is_keyframe: false,
            config: None,
        });
    }
    // show_frame and error_resilient_mode
    reader.read_bits(2)?;
    ensure!(
        reader.read_bits(24)? == SYNC_CODE,
        "VP9 keyframe without its sync code"
    );
    let bit_depth = match profile {
        0 | 1 => 8,
        _ if reader.read_bit()? => 12,
        _ => 10,
    };
    let has_chroma_subsampling_bits = profile == 1 || profile == 3;
    let (full_range, subsampling_x, subsampling_y) = if reader.read_bits(3)? != CS_RGB {
        let full_range = reader.read_bit()?;
        if has_chroma_subsampling_bits {
            let subsampling = (reader.read_bit()?, reader.read_bit()?);
            reader.read_bits(1)?;
            (full_range, subsampling.0, subsampling.1)
        } else {
            (full_range, true, true)
        }
    } else {
        if has_chroma_subsampling_bits {
            reader.read_bits(1)?;
        }
        (true, false, false)
    };
    let width = reader.read_bits(16)? as u32 + 1;
    let height = reader.read_bits(16)? as u32 + 1;
    Ok(Vp9Frame {
        is_keyframe: true,
        config: Some(Vp9Config {
            profile,
            bit_depth,
            subsampling_x,
            subsampling_y,
            full_range,
            width,
            height,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FIXTURE_VP9_IVF, ivf_frames};

    #[test]
    fn reads_the_configuration_of_a_keyframe() {
        // Arrange
        let frames = ivf_frames(FIXTURE_VP9_IVF);

        // Act
        let frame = parse_frame(&frames[0]).unwrap();

        // Assert
        assert!(frame.is_keyframe);
        assert_eq!(
            frame.config,
            Some(Vp9Config {
                profile: 0,
                bit_depth: 8,
                subsampling_x: true,
                subsampling_y: true,
                full_range: false,
                width: 64,
                height: 48
            })
        );
    }

    #[test]
    fn reads_the_bit_depth_of_a_profile_2_keyframe() {
        // Arrange: the header of a 64x48 10-bit 4:2:0 keyframe from libvpx-vp9
        let header = [0x92, 0x49, 0x83, 0x42, 0x00, 0x01, 0xf8, 0x01, 0x7b, 0x03];

        // Act
        let config = parse_frame(&header).unwrap().config.unwrap();

        // Assert
        assert_eq!(
            (
                config.profile,
                config.bit_depth,
                config.width,
                config.height
            ),
            (2, 10, 64, 48)
        );
    }

    #[test]
    fn reports_an_inter_frame_without_a_configuration() {
        // Arrange
        let frames = ivf_frames(FIXTURE_VP9_IVF);

        // Act
        let frame = parse_frame(&frames[1]).unwrap();

        // Assert
        assert!(!frame.is_keyframe);
        assert_eq!(frame.config, None);
    }
}
