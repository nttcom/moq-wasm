use anyhow::Context;
use bytes::BytesMut;

use crate::modules::extensions::buf_get_ext::BufGetExt;
use crate::modules::extensions::buf_put_ext::BufPutExt;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct Location {
    pub group_id: u64,
    pub object_id: u64,
}

impl Location {
    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let group_id = buf.try_get_varint().context("location group id")?;
        let object_id = buf.try_get_varint().context("location object id")?;
        Ok(Self {
            group_id,
            object_id,
        })
    }

    pub fn encode(&self) -> anyhow::Result<BytesMut> {
        let mut payload = BytesMut::new();
        payload.put_varint(self.group_id)?;
        payload.put_varint(self.object_id)?;
        Ok(payload)
    }

    /// The last Location: [`Location::next`] returns `None` after it.
    pub const MAX: Self = Self {
        group_id: u64::MAX,
        object_id: u64::MAX,
    };

    /// The first Location after `self`: the next object of its group, or the
    /// start of the next group after the largest Object ID. `None` after
    /// [`Location::MAX`].
    pub fn next(self) -> Option<Self> {
        match self.object_id.checked_add(1) {
            Some(object_id) => Some(Self {
                group_id: self.group_id,
                object_id,
            }),
            None => Some(Self {
                group_id: self.group_id.checked_add(1)?,
                object_id: 0,
            }),
        }
    }

    /// The End Location of a FETCH range whose last object is `self`. The
    /// largest Object ID maps to Object 0, the entire group (draft-18 §10.12.1).
    pub fn end_after(self) -> Self {
        Self {
            group_id: self.group_id,
            object_id: self.object_id.checked_add(1).unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    mod success {
        use bytes::BytesMut;

        use crate::modules::moqt::control_plane::control_messages::messages::parameters::location::Location;

        #[test]
        fn packetize_and_depacketize() {
            let location = Location {
                group_id: 10,
                object_id: 5,
            };

            let buf = location.encode().unwrap();
            let mut cursor = std::io::Cursor::new(&buf[..]);

            // depacketize
            let depacketized_location = Location::decode(&mut cursor).unwrap();

            assert_eq!(location.group_id, depacketized_location.group_id);
            assert_eq!(location.object_id, depacketized_location.object_id);
        }

        #[test]
        fn packetize_check_bytes() {
            let location = Location {
                group_id: 10,
                object_id: 5,
            };

            let mut buf = BytesMut::new();
            buf.extend(location.encode().unwrap());

            let expected_bytes = vec![10, 5];
            assert_eq!(buf.as_ref(), expected_bytes.as_slice());
        }

        #[test]
        fn next_is_the_following_object_of_the_same_group() {
            // Act / Assert
            assert_eq!(
                Location {
                    group_id: 10,
                    object_id: 5
                }
                .next(),
                Some(Location {
                    group_id: 10,
                    object_id: 6
                })
            );
        }

        #[test]
        fn next_after_the_largest_object_id_is_the_next_group_start() {
            // Act / Assert
            assert_eq!(
                Location {
                    group_id: 10,
                    object_id: u64::MAX
                }
                .next(),
                Some(Location {
                    group_id: 11,
                    object_id: 0
                })
            );
        }

        #[test]
        fn nothing_follows_the_last_location() {
            // Act / Assert
            assert_eq!(Location::MAX.next(), None);
        }

        #[test]
        fn end_after_is_the_following_object_id() {
            // Act / Assert
            assert_eq!(
                Location {
                    group_id: 10,
                    object_id: 5
                }
                .end_after(),
                Location {
                    group_id: 10,
                    object_id: 6
                }
            );
        }

        #[test]
        fn end_after_the_largest_object_id_is_the_entire_group() {
            // Act / Assert
            assert_eq!(
                Location {
                    group_id: 10,
                    object_id: u64::MAX
                }
                .end_after(),
                Location {
                    group_id: 10,
                    object_id: 0
                }
            );
        }
    }

    mod properties {
        use proptest::prelude::*;

        use crate::modules::moqt::control_plane::control_messages::messages::parameters::location::Location;

        /// Favours the edges an ID reaches: 0, 1, the varint limit of
        /// draft-14 (2^62) and the largest vi64 value.
        fn id() -> impl Strategy<Value = u64> {
            prop_oneof![
                Just(0),
                Just(1),
                Just((1 << 62) - 1),
                Just(1 << 62),
                Just(u64::MAX - 1),
                Just(u64::MAX),
                any::<u64>(),
            ]
        }

        fn location() -> impl Strategy<Value = Location> {
            (id(), id()).prop_map(|(group_id, object_id)| Location {
                group_id,
                object_id,
            })
        }

        proptest! {
            #[test]
            fn next_is_the_least_location_after_self(at in location(), other in location()) {
                match at.next() {
                    Some(next) => {
                        prop_assert!(next > at);
                        prop_assert!(other <= at || other >= next);
                    }
                    None => prop_assert!(other <= at),
                }
            }
        }
    }
}
