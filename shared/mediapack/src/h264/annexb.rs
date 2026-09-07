use bytes::{BufMut, Bytes, BytesMut};

use crate::h264::avcc::length_prefixed;

pub const START_CODE: [u8; 4] = [0, 0, 0, 1];

pub fn nal_units(data: &[u8]) -> NalUnits<'_> {
    NalUnits { data, position: 0 }
}

pub struct NalUnits<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Iterator for NalUnits<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let start_code = find_start_code(self.data, self.position)?;
            let body_start = start_code + 3;
            let end = find_start_code(self.data, body_start).unwrap_or(self.data.len());
            self.position = end;
            let nal = trim_trailing_zeros(&self.data[body_start..end]);
            if !nal.is_empty() {
                return Some(nal);
            }
        }
    }
}

fn find_start_code(data: &[u8], from: usize) -> Option<usize> {
    data.get(from..)?
        .windows(3)
        .position(|window| window == [0, 0, 1])
        .map(|offset| from + offset)
}

fn trim_trailing_zeros(nal: &[u8]) -> &[u8] {
    let end = nal.iter().rposition(|byte| *byte != 0).map_or(0, |i| i + 1);
    &nal[..end]
}

pub fn with_start_codes<'a>(nals: impl IntoIterator<Item = &'a [u8]>) -> Bytes {
    let mut out = BytesMut::new();
    for nal in nals {
        out.put_slice(&START_CODE);
        out.put_slice(nal);
    }
    out.freeze()
}

pub fn annexb_to_avcc(data: &[u8], nal_length_size: usize) -> Bytes {
    length_prefixed(nal_units(data), nal_length_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_nal_units_with_three_and_four_byte_start_codes() {
        // Arrange
        let stream = [
            0, 0, 0, 1, 0x67, 0xAA, 0, 0, 1, 0x68, 0xBB, 0, 0, 0, 1, 0x65, 0xCC, 0xDD,
        ];

        // Act
        let nals: Vec<&[u8]> = nal_units(&stream).collect();

        // Assert
        assert_eq!(
            nals,
            [&[0x67, 0xAA][..], &[0x68, 0xBB], &[0x65, 0xCC, 0xDD]]
        );
    }

    #[test]
    fn ignores_leading_garbage_and_empty_units() {
        // Arrange
        let stream = [0xFF, 0, 0, 1, 0, 0, 1, 0x65, 0x01];

        // Act
        let nals: Vec<&[u8]> = nal_units(&stream).collect();

        // Assert
        assert_eq!(nals, [&[0x65, 0x01][..]]);
    }

    #[test]
    fn converts_annexb_to_length_prefixed_avcc() {
        // Arrange
        let stream = [0, 0, 0, 1, 0x65, 0xCC, 0, 0, 1, 0x41, 0xDD, 0xEE];

        // Act
        let avcc = annexb_to_avcc(&stream, 4);

        // Assert
        assert_eq!(
            avcc.as_ref(),
            [0, 0, 0, 2, 0x65, 0xCC, 0, 0, 0, 3, 0x41, 0xDD, 0xEE]
        );
    }

    #[test]
    fn joins_nal_units_with_start_codes() {
        // Arrange
        let nals = [&[0x67, 0x01][..], &[0x68, 0x02]];

        // Act
        let stream = with_start_codes(nals);

        // Assert
        assert_eq!(
            stream.as_ref(),
            [0, 0, 0, 1, 0x67, 0x01, 0, 0, 0, 1, 0x68, 0x02]
        );
    }
}
