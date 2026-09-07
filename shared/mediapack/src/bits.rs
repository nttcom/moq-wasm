use anyhow::{Result, ensure};

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub(crate) fn read_bit(&mut self) -> Result<bool> {
        let byte = self
            .data
            .get(self.position / 8)
            .ok_or_else(|| anyhow::anyhow!("bit reader exhausted at bit {}", self.position))?;
        let bit = (byte >> (7 - self.position % 8)) & 1 == 1;
        self.position += 1;
        Ok(bit)
    }

    pub(crate) fn read_bits(&mut self, count: u32) -> Result<u64> {
        ensure!(count <= 64, "cannot read {count} bits at once");
        let mut value = 0_u64;
        for _ in 0..count {
            value = (value << 1) | self.read_bit()? as u64;
        }
        Ok(value)
    }

    pub(crate) fn read_ue(&mut self) -> Result<u64> {
        let mut leading_zeros = 0_u32;
        while !self.read_bit()? {
            leading_zeros += 1;
            ensure!(leading_zeros <= 32, "exp-golomb prefix too long");
        }
        let suffix = self.read_bits(leading_zeros)?;
        Ok((1_u64 << leading_zeros) - 1 + suffix)
    }

    pub(crate) fn read_se(&mut self) -> Result<i64> {
        let code = self.read_ue()? as i64;
        Ok(if code % 2 == 1 {
            (code + 1) / 2
        } else {
            -(code / 2)
        })
    }
}

pub(crate) struct BitWriter {
    bytes: Vec<u8>,
    bit_count: usize,
}

impl BitWriter {
    pub(crate) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            bit_count: 0,
        }
    }

    pub(crate) fn write_bits(&mut self, value: u64, count: u32) {
        for shift in (0..count).rev() {
            if self.bit_count.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let bit = ((value >> shift) & 1) as u8;
            let last = self.bytes.len() - 1;
            self.bytes[last] |= bit << (7 - self.bit_count % 8);
            self.bit_count += 1;
        }
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_exp_golomb_codes() {
        // Arrange: "1" "010" "011" "00100" -> 0, 1, 2, 3
        let mut reader = BitReader::new(&[0b1010_0110, 0b0100_0000]);

        // Act
        let values: Vec<u64> = (0..4).map(|_| reader.read_ue().unwrap()).collect();

        // Assert
        assert_eq!(values, [0, 1, 2, 3]);
    }

    #[test]
    fn reads_signed_exp_golomb_codes() {
        // Arrange: "010" -> +1, "011" -> -1
        let mut reader = BitReader::new(&[0b0100_1100]);

        // Act
        let first = reader.read_se().unwrap();
        let second = reader.read_se().unwrap();

        // Assert
        assert_eq!((first, second), (1, -1));
    }

    #[test]
    fn writer_round_trips_through_reader() {
        // Arrange
        let mut writer = BitWriter::new();
        writer.write_bits(0b10110, 5);
        writer.write_bits(0x3, 4);
        writer.write_bits(0x1ABCD, 17);

        // Act
        let bytes = writer.finish();
        let mut reader = BitReader::new(&bytes);

        // Assert
        assert_eq!(bytes.len(), 4);
        assert_eq!(reader.read_bits(5).unwrap(), 0b10110);
        assert_eq!(reader.read_bits(4).unwrap(), 0x3);
        assert_eq!(reader.read_bits(17).unwrap(), 0x1ABCD);
    }
}
