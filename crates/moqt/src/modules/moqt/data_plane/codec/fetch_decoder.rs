use bytes::BytesMut;
use tokio_util::codec::Decoder;

use crate::modules::moqt::data_plane::object::{
    decode_error::DecodeError,
    fetch::{FetchHeader, FetchObjectField},
};

/// One frame of a FETCH stream as draft-14 encodes it.
#[derive(Debug)]
pub(crate) enum FetchFrame {
    Header(FetchHeader),
    Object(FetchObjectField),
}

#[derive(Debug)]
pub(crate) struct FetchDecoder {
    header_received: bool,
}

impl Decoder for FetchDecoder {
    type Item = FetchFrame;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if !self.header_received {
            self.decode_header(src)
        } else {
            self.decode_object(src)
        }
    }
}

impl FetchDecoder {
    pub(crate) fn new() -> Self {
        Self {
            header_received: false,
        }
    }

    fn decode_header(&mut self, src: &mut BytesMut) -> Result<Option<FetchFrame>, std::io::Error> {
        let mut cursor = std::io::Cursor::new(&src[..]);
        match FetchHeader::decode(&mut cursor) {
            Ok(header) => {
                let pos = cursor.position() as usize;
                let _ = src.split_to(pos);
                self.header_received = true;
                Ok(Some(FetchFrame::Header(header)))
            }
            Err(DecodeError::NeedMoreData) => Ok(None),
            Err(e) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to decode fetch header: {:?}", e),
            )),
        }
    }

    fn decode_object(&self, src: &mut BytesMut) -> Result<Option<FetchFrame>, std::io::Error> {
        if src.is_empty() {
            return Ok(None);
        }
        match FetchObjectField::decode(src) {
            Ok(fields) => Ok(Some(FetchFrame::Object(fields))),
            Err(DecodeError::NeedMoreData) => Ok(None),
            Err(e) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to decode fetch object fields: {:?}", e),
            )),
        }
    }
}
