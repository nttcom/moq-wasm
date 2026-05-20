use bytes::BytesMut;
use tokio_util::codec::Decoder;

use crate::modules::extensions::buf_get_ext::BufGetExt;

use crate::Subgroup;
use crate::modules::moqt::data_plane::{
    codec::{fetch_decoder::FetchDecoder, subgroup_decoder::SubgroupDecoder},
    object::fetch::FetchHeader,
    stream::fetch_data_receiver::Fetch,
};

#[derive(Debug)]
pub enum UniStreamData {
    Subgroup(Subgroup),
    Fetch(Fetch),
}

#[derive(Debug)]
pub(crate) struct UniStreamDecoder {
    inner: InnerDecoder,
}

#[derive(Debug)]
enum InnerDecoder {
    Unknown,
    Subgroup(SubgroupDecoder),
    Fetch(FetchDecoder),
}

impl Decoder for UniStreamDecoder {
    type Item = UniStreamData;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // Determine the decoder type from the first message type.
        if let InnerDecoder::Unknown = self.inner {
            let mut cursor = std::io::Cursor::new(&src[..]);
            let message_type = match cursor.try_get_varint() {
                Ok(b) => b,
                Err(_) => return Ok(None),
            };
            if (0x10..=0x1d).contains(&message_type) {
                // 0x10..=0x1d are valid SubgroupHeader message types.
                self.inner = InnerDecoder::Subgroup(SubgroupDecoder::new());
            } else if message_type == FetchHeader::TYPE {
                self.inner = InnerDecoder::Fetch(FetchDecoder::new());
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Unknown message type: {message_type:#x}"),
                ));
            }
        }
        // Decode using the inner decoder chosen above.
        match &mut self.inner {
            InnerDecoder::Subgroup(dec) => match dec.decode(src)? {
                Some(subgroup) => Ok(Some(UniStreamData::Subgroup(subgroup))),
                None => Ok(None),
            },
            InnerDecoder::Fetch(dec) => match dec.decode(src)? {
                Some(item) => Ok(Some(UniStreamData::Fetch(item))),
                None => Ok(None),
            },
            InnerDecoder::Unknown => unreachable!(),
        }
    }
}

impl UniStreamDecoder {
    pub(crate) fn new() -> Self {
        Self {
            inner: InnerDecoder::Unknown,
        }
    }
}
