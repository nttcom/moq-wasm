//! Splits an H.264 Annex B stream into access units, the frames
//! `MediaSource::push_video` takes.

const NAL_SLICE: u8 = 1;
const NAL_IDR_SLICE: u8 = 5;
const NAL_SPS: u8 = 7;

/// Each access unit keeps its start codes. One starts at a picture's first
/// slice (`first_mb_in_slice` 0) or at a non-slice NAL unit after a slice,
/// such as the SPS before a keyframe (H.264 §7.4.1.2.3).
pub fn access_units(stream: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    let mut unit_start = 0;
    let mut has_slice = false;
    for (start, nal) in nal_units(stream) {
        let Some(&header) = nal.first() else {
            continue;
        };
        let nal_type = header & 0x1f;
        let is_slice = nal_type == NAL_SLICE || nal_type == NAL_IDR_SLICE;
        // first_mb_in_slice is ue(v), which codes 0 as the single bit 1.
        let starts_picture = is_slice && nal.get(1).is_some_and(|byte| byte & 0x80 != 0);
        if has_slice && (starts_picture || !is_slice) {
            units.push(&stream[unit_start..start]);
            unit_start = start;
            has_slice = false;
        }
        has_slice |= is_slice;
    }
    if unit_start < stream.len() {
        units.push(&stream[unit_start..]);
    }
    units
}

/// The WebCodecs codec string (`avc1.PPCCLL`) of the stream's first SPS.
pub fn codec_string(stream: &[u8]) -> Option<String> {
    nal_units(stream).find_map(|(_, nal)| {
        let sps = nal.get(..4).filter(|sps| sps[0] & 0x1f == NAL_SPS)?;
        Some(format!("avc1.{:02x}{:02x}{:02x}", sps[1], sps[2], sps[3]))
    })
}

/// Each NAL unit with the offset of the start code before it.
fn nal_units(stream: &[u8]) -> impl Iterator<Item = (usize, &[u8])> {
    let starts: Vec<(usize, usize)> = start_codes(stream).collect();
    (0..starts.len()).map(move |index| {
        let (start, payload) = starts[index];
        let end = starts.get(index + 1).map_or(stream.len(), |next| next.0);
        (start, &stream[payload..end])
    })
}

/// The offset of each start code and of the NAL unit after it; a 3-byte
/// start code preceded by a zero byte is a 4-byte one.
fn start_codes(stream: &[u8]) -> impl Iterator<Item = (usize, usize)> + '_ {
    stream
        .windows(3)
        .enumerate()
        .filter(|(_, window)| *window == [0, 0, 1])
        .map(move |(offset, _)| {
            let start = if offset > 0 && stream[offset - 1] == 0 {
                offset - 1
            } else {
                offset
            };
            (start, offset + 3)
        })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const SPS: &[u8] = &[0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0xaa];
    const PPS: &[u8] = &[0, 0, 0, 1, 0x68, 0xce];
    const IDR: &[u8] = &[0, 0, 1, 0x65, 0x88, 0x01];
    const IDR_SECOND_SLICE: &[u8] = &[0, 0, 1, 0x65, 0x40, 0x02];
    const P: &[u8] = &[0, 0, 0, 1, 0x41, 0x9a, 0x03];

    #[test]
    fn a_keyframe_keeps_its_parameter_sets_and_every_slice() {
        // Arrange
        let stream = [SPS, PPS, IDR, IDR_SECOND_SLICE, P, P].concat();

        // Act
        let units = access_units(&stream);

        // Assert
        assert_eq!(
            units,
            vec![[SPS, PPS, IDR, IDR_SECOND_SLICE].concat().as_slice(), P, P]
        );
    }

    #[test]
    fn the_codec_string_comes_from_the_sps() {
        // Act / Assert
        assert_eq!(
            codec_string(&[SPS, PPS, IDR].concat()).as_deref(),
            Some("avc1.42c01f")
        );
    }

    proptest! {
        #[test]
        fn the_access_units_add_up_to_the_stream(
            picture_slices in proptest::collection::vec(1usize..4, 1..12),
            keyframe_every in 1usize..5,
        ) {
            // Arrange
            let mut parts: Vec<&[u8]> = Vec::new();
            for (index, slices) in picture_slices.iter().enumerate() {
                if index % keyframe_every == 0 {
                    parts.extend([SPS, PPS, IDR]);
                    parts.extend(std::iter::repeat_n(IDR_SECOND_SLICE, slices - 1));
                } else {
                    parts.extend(std::iter::repeat_n(P, *slices));
                }
            }
            let stream = parts.concat();

            // Act
            let units = access_units(&stream);

            // Assert: one unit per picture, together the whole stream
            prop_assert_eq!(units.concat(), stream.clone());
            let p_units = picture_slices
                .iter()
                .enumerate()
                .filter(|(index, _)| index % keyframe_every != 0)
                .map(|(_, slices)| slices)
                .sum::<usize>();
            let keyframes = picture_slices.len().div_ceil(keyframe_every);
            prop_assert_eq!(units.len(), keyframes + p_units);
        }
    }
}
