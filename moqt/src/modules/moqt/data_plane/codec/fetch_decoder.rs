use bytes::BytesMut;
use tokio_util::codec::Decoder;

use crate::modules::moqt::data_plane::object::{
    decode_error::DecodeError,
    fetch::{FetchHeader, FetchObjectField},
};
use crate::modules::moqt::data_plane::stream::fetch_data_receiver::Fetch;

#[derive(Debug)]
pub(crate) struct FetchDecoder {
    header_received: bool,
}

impl Decoder for FetchDecoder {
    type Item = Fetch;
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

    fn decode_header(&mut self, src: &mut BytesMut) -> Result<Option<Fetch>, std::io::Error> {
        let mut cursor = std::io::Cursor::new(&src[..]);
        match FetchHeader::decode(&mut cursor) {
            Ok(header) => {
                let pos = cursor.position() as usize;
                let _ = src.split_to(pos);
                self.header_received = true;
                Ok(Some(Fetch::Header(header)))
            }
            Err(DecodeError::NeedMoreData) => Ok(None),
            Err(e) => {
                tracing::error!("Failed to decode fetch header: {:?}", e);
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to decode fetch header: {:?}", e),
                ))
            }
        }
    }

    fn decode_object(&self, src: &mut BytesMut) -> Result<Option<Fetch>, std::io::Error> {
        match FetchObjectField::decode(src) {
            Ok(fields) => Ok(Some(Fetch::Object(fields))),
            Err(DecodeError::NeedMoreData) => Ok(None),
            Err(e) => {
                tracing::error!("Failed to decode fetch object fields: {:?}", e);
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to decode fetch object fields: {:?}", e),
                ))
            }
        }
    }
}
