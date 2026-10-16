use anyhow::{Result, ensure};

const START_CODE: [u8; 3] = [0x9d, 0x01, 0x2a];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vp8Config {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub struct Vp8Frame {
    pub is_keyframe: bool,
    /// Present on keyframes only; inter frames keep the keyframe's size.
    pub config: Option<Vp8Config>,
}

/// Reads the frame tag and, on a keyframe, the frame size (RFC 6386 §9.1).
pub fn parse_frame(data: &[u8]) -> Result<Vp8Frame> {
    ensure!(data.len() >= 3, "VP8 frame shorter than its frame tag");
    let is_keyframe = data[0] & 1 == 0;
    if !is_keyframe {
        return Ok(Vp8Frame {
            is_keyframe,
            config: None,
        });
    }
    ensure!(data.len() >= 10, "VP8 keyframe shorter than its header");
    ensure!(
        data[3..6] == START_CODE,
        "VP8 keyframe without its start code"
    );
    let dimension =
        |offset: usize| u32::from(u16::from_le_bytes([data[offset], data[offset + 1]]) & 0x3fff);
    Ok(Vp8Frame {
        is_keyframe,
        config: Some(Vp8Config {
            width: dimension(6),
            height: dimension(8),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FIXTURE_VP8_IVF, ivf_frames};

    #[test]
    fn reads_the_size_of_a_keyframe() {
        // Arrange
        let frames = ivf_frames(FIXTURE_VP8_IVF);

        // Act
        let frame = parse_frame(&frames[0]).unwrap();

        // Assert
        assert!(frame.is_keyframe);
        assert_eq!(
            frame.config,
            Some(Vp8Config {
                width: 64,
                height: 48
            })
        );
    }

    #[test]
    fn reports_an_inter_frame_without_a_configuration() {
        // Arrange
        let frames = ivf_frames(FIXTURE_VP8_IVF);

        // Act
        let frame = parse_frame(&frames[1]).unwrap();

        // Assert
        assert!(!frame.is_keyframe);
        assert_eq!(frame.config, None);
    }

    #[test]
    fn rejects_a_keyframe_without_the_start_code() {
        // Arrange
        let mut data = ivf_frames(FIXTURE_VP8_IVF)[0].to_vec();
        data[3] = 0;

        // Act
        let error = parse_frame(&data).unwrap_err();

        // Assert
        assert_eq!(error.to_string(), "VP8 keyframe without its start code");
    }
}
