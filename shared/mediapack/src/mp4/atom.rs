use anyhow::{Result, ensure};

pub(crate) const HEADER_LENGTH: usize = 8;

pub(crate) struct Atom<'a> {
    pub kind: &'a [u8],
    pub payload: &'a [u8],
}

pub(crate) struct Atoms<'a> {
    data: &'a [u8],
}

pub(crate) fn atoms(data: &[u8]) -> Atoms<'_> {
    Atoms { data }
}

impl<'a> Iterator for Atoms<'a> {
    type Item = Atom<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let (size, kind) = peek(self.data)?;
        if size > self.data.len() {
            return None;
        }
        let atom = Atom {
            kind,
            payload: &self.data[HEADER_LENGTH..size],
        };
        self.data = &self.data[size..];
        Some(atom)
    }
}

/// Returns the size and kind of the atom starting at `data`, once its header is
/// complete. A size of zero means "extends to the end of the file", which
/// fragmented streams do not use.
pub(crate) fn peek(data: &[u8]) -> Option<(usize, &[u8])> {
    if data.len() < HEADER_LENGTH {
        return None;
    }
    let size = u32::from_be_bytes(data[..4].try_into().ok()?) as usize;
    if size < HEADER_LENGTH {
        return None;
    }
    Some((size, &data[4..HEADER_LENGTH]))
}

pub(crate) fn find<'a>(payload: &'a [u8], kind: &[u8]) -> Option<Atom<'a>> {
    atoms(payload).find(|atom| atom.kind == kind)
}

pub(crate) struct FullAtom<'a> {
    pub flags: u32,
    pub body: &'a [u8],
    version: u8,
}

impl FullAtom<'_> {
    pub fn is_version_one(&self) -> bool {
        self.version == 1
    }
}

pub(crate) fn full_atom<'a>(payload: &'a [u8], kind: &str) -> Result<FullAtom<'a>> {
    ensure!(payload.len() >= 4, "{kind} is shorter than its header");
    Ok(FullAtom {
        version: payload[0],
        flags: u32::from_be_bytes([0, payload[1], payload[2], payload[3]]),
        body: &payload[4..],
    })
}

pub(crate) fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow::anyhow!("32-bit field at offset {offset} is truncated"))?;
    Ok(u32::from_be_bytes(bytes.try_into()?))
}

pub(crate) fn read_u64(data: &[u8], offset: usize) -> Result<u64> {
    let bytes = data
        .get(offset..offset + 8)
        .ok_or_else(|| anyhow::anyhow!("64-bit field at offset {offset} is truncated"))?;
    Ok(u64::from_be_bytes(bytes.try_into()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_sibling_atoms() {
        // Arrange
        let data = [
            0, 0, 0, 9, b'f', b't', b'y', b'p', 1, 0, 0, 0, 10, b'm', b'd', b'a', b't', 2, 3,
        ];

        // Act
        let found: Vec<(&[u8], &[u8])> =
            atoms(&data).map(|atom| (atom.kind, atom.payload)).collect();

        // Assert
        assert_eq!(
            found,
            [(&b"ftyp"[..], &[1][..]), (&b"mdat"[..], &[2, 3][..])]
        );
    }

    #[test]
    fn stops_before_an_incomplete_atom() {
        // Arrange
        let data = [0, 0, 0, 12, b'm', b'o', b'o', b'f', 1, 2];

        // Act / Assert
        assert!(atoms(&data).next().is_none());
        assert_eq!(peek(&data), Some((12, &b"moof"[..])));
    }

    #[test]
    fn reads_the_version_and_flags_of_a_full_atom() {
        // Arrange
        let payload = [1, 0x00, 0x02, 0x01, 9];

        // Act
        let atom = full_atom(&payload, "tfhd").unwrap();

        // Assert
        assert!(atom.is_version_one());
        assert_eq!(atom.flags, 0x0201);
        assert_eq!(atom.body, [9]);
    }
}
