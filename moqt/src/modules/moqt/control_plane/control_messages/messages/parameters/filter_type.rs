use bytes::{Buf, BufMut, BytesMut};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::messages::parameters::location::Location,
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
    LargestObject,
    NextGroupStart,
    AbsoluteStart { location: Location },
    AbsoluteRange { location: Location, end_group: u64 },
}

impl FilterType {
    /// draft-14 §9.7: the Start Location of a subscription with this filter,
    /// given the Largest Location of its SUBSCRIBE_OK (`None` when no content
    /// has been delivered yet).
    pub fn start_location(&self, largest: Option<Location>) -> Location {
        match (self, largest) {
            (Self::AbsoluteStart { location } | Self::AbsoluteRange { location, .. }, _) => {
                *location
            }
            (Self::LargestObject, Some(largest)) => Location {
                group_id: largest.group_id,
                object_id: largest.object_id + 1,
            },
            (Self::NextGroupStart, Some(largest)) => Location {
                group_id: largest.group_id + 1,
                object_id: 0,
            },
            (Self::LargestObject | Self::NextGroupStart, None) => Location {
                group_id: 0,
                object_id: 0,
            },
        }
    }

    pub fn decode(bytes: &mut std::io::Cursor<&[u8]>) -> Option<Self> {
        let value = FilterTypeValue::try_from(bytes.get_u8()).ok()?;
        match value {
            FilterTypeValue::LatestObject => Some(FilterType::LargestObject),
            FilterTypeValue::LatestGroup => Some(FilterType::NextGroupStart),
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

    pub fn encode(&self) -> BytesMut {
        let mut payload = BytesMut::new();
        match self {
            FilterType::LargestObject => {
                payload.put_u8(FilterTypeValue::LatestObject as u8);
                payload
            }
            FilterType::NextGroupStart => {
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

#[cfg(test)]
mod tests {
    use super::FilterType;
    use crate::modules::moqt::control_plane::control_messages::messages::parameters::location::Location;

    fn location(group_id: u64, object_id: u64) -> Location {
        Location {
            group_id,
            object_id,
        }
    }

    #[test]
    fn largest_object_starts_after_the_largest_location() {
        // Act
        let start = FilterType::LargestObject.start_location(Some(location(5, 3)));

        // Assert
        assert_eq!(start, location(5, 4));
    }

    #[test]
    fn next_group_start_starts_at_the_next_group() {
        // Act
        let start = FilterType::NextGroupStart.start_location(Some(location(5, 3)));

        // Assert
        assert_eq!(start, location(6, 0));
    }

    #[test]
    fn filter_without_delivered_content_starts_at_the_beginning() {
        // Act
        let start = FilterType::NextGroupStart.start_location(None);

        // Assert
        assert_eq!(start, location(0, 0));
    }

    #[test]
    fn absolute_start_keeps_the_requested_location_below_the_largest() {
        // Arrange
        let filter_type = FilterType::AbsoluteStart {
            location: location(2, 0),
        };

        // Act
        let start = filter_type.start_location(Some(location(5, 3)));

        // Assert
        assert_eq!(start, location(2, 0));
    }
}
