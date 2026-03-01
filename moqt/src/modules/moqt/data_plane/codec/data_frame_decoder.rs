use bytes::BytesMut;
use tokio_util::codec::Decoder;

use crate::{
    Subgroup, SubgroupHeader, SubgroupHeaderType, SubgroupObjectField,
    modules::moqt::data_plane::object::decode_error::DecodeError,
};

#[derive(Debug)]
pub(crate) struct DataFrameDecoder {
    subgroup_header_type: Option<SubgroupHeaderType>,
}

impl Decoder for DataFrameDecoder {
    type Item = Subgroup;
    // TODO: define a proper error type.
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if self.subgroup_header_type.is_none() {
            let mut cursor_buf = std::io::Cursor::new(&src[..]);
            let subgroup = self.decode_header(&mut cursor_buf)?;
            let pos = cursor_buf.position() as usize;
            let _ = src.split_to(pos);
            Ok(subgroup)
        } else {
            self.decode_object_field(src)
        }
    }
}

impl DataFrameDecoder {
    pub(crate) fn new() -> Self {
        Self {
            subgroup_header_type: None,
        }
    }

    fn decode_header(
        &mut self,
        cursor: &mut std::io::Cursor<&[u8]>,
    ) -> Result<Option<Subgroup>, std::io::Error> {
        // The first frame should always be a subgroup header.
        let subgroup_header = match SubgroupHeader::decode(cursor) {
            Ok(header) => header,
            Err(e) => {
                match e {
                    DecodeError::NeedMoreData => {
                        // Not enough data to decode the header, wait for more data.
                        return Ok(None);
                    }
                    _ => {
                        // Log the error and return it.
                        tracing::error!("Failed to decode subgroup header: {:?}", e);
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("Failed to decode subgroup header: {:?}", e),
                        ));
                    }
                }
            }
        };
        self.subgroup_header_type = Some(subgroup_header.message_type);
        Ok(Some(Subgroup::Header(subgroup_header)))
    }

    fn decode_object_field(&self, src: &mut BytesMut) -> Result<Option<Subgroup>, std::io::Error> {
        let subgroup_object_field =
            match SubgroupObjectField::decode(self.subgroup_header_type.unwrap(), src) {
                Ok(field) => field,
                Err(e) => {
                    match e {
                        DecodeError::NeedMoreData => {
                            // Not enough data to decode the object field, wait for more data.
                            return Ok(None);
                        }
                        _ => {
                            // Log the error and return it.
                            tracing::error!("Failed to decode subgroup object field: {:?}", e);
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("Failed to decode subgroup object field: {:?}", e),
                            ));
                        }
                    }
                }
            };
        Ok(Some(Subgroup::Object(subgroup_object_field)))
    }
}
