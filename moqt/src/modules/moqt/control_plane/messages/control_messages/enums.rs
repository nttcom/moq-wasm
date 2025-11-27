use bytes::{Buf, BufMut, BytesMut};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::{
    Location,
    modules::{
        extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
        moqt::control_plane::messages::control_messages::util::u8_to_bool,
    },
};

#[derive(Debug, Clone, TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
enum FilterTypeValue {
    LatestGroup = 0x01,
    LatestObject = 0x02,
    AbsoluteStart = 0x03,
    AbsoluteRange = 0x04,
}

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum FilterType {
    LatestObject,
    LatestGroup,
    AbsoluteStart { location: Location },
    AbsoluteRange { location: Location, end_group: u64 },
}

impl FilterType {
    pub(crate) fn decode(bytes: &mut BytesMut) -> Option<Self> {
        let value = FilterTypeValue::try_from(bytes.get_u8()).ok()?;
        match value {
            FilterTypeValue::LatestObject => Some(FilterType::LatestObject),
            FilterTypeValue::LatestGroup => Some(FilterType::LatestGroup),
            FilterTypeValue::AbsoluteStart => {
                let start_location = Location::decode(bytes)?;
                Some(FilterType::AbsoluteStart {
                    location: start_location,
                })
            }
            FilterTypeValue::AbsoluteRange => {
                let start_location = Location::decode(bytes)?;
                let end_group = bytes.try_get_varint().log_context("end group").ok()?;
                Some(FilterType::AbsoluteRange {
                    location: start_location,
                    end_group,
                })
            }
        }
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        match self {
            FilterType::LatestObject => {
                payload.put_u8(FilterTypeValue::LatestObject as u8);
                payload
            }
            FilterType::LatestGroup => {
                payload.put_u8(FilterTypeValue::LatestGroup as u8);
                payload
            }
            FilterType::AbsoluteStart { location } => {
                payload.put_u8(FilterTypeValue::AbsoluteStart as u8);
                let bytes = location.encode();
                payload.unsplit(bytes);
                payload
            }
            FilterType::AbsoluteRange {
                location,
                end_group,
            } => {
                payload.put_u8(FilterTypeValue::AbsoluteRange as u8);
                let bytes = location.encode();
                payload.unsplit(bytes);
                payload.put_varint(*end_group);
                payload
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum ContentExists {
    False,
    True { location: Location },
}

impl ContentExists {
    pub(crate) fn decode(bytes: &mut BytesMut) -> Option<Self> {
        let value = bytes.get_u8();
        let content_exists = u8_to_bool(value).ok()?;
        if content_exists {
            let location = Location::decode(bytes)?;
            Some(ContentExists::True { location })
        } else {
            Some(ContentExists::False)
        }
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        match self {
            ContentExists::False => {
                payload.put_u8(0);
            }
            ContentExists::True { location } => {
                payload.put_u8(1);
                let bytes = location.encode();
                payload.unsplit(bytes);
            }
        }
        payload
    }
}
