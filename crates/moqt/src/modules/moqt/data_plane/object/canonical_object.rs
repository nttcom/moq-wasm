use bytes::Bytes;

use crate::modules::moqt::{
    control_plane::control_messages::messages::parameters::location::Location,
    data_plane::object::{extension_headers::ExtensionHeaders, object_status::ObjectStatus},
};

/// How an object travels (draft-14 §2.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardingPreference {
    Subgroup { subgroup_id: u64 },
    Datagram,
}

/// An object with the properties the data model gives it (draft-14 §2.1,
/// §10.2.1), independent of how a draft lays it out on a subgroup stream, a
/// datagram or a FETCH response. A zero-length object carries its meaning in
/// `status`; an object with a payload is `Normal`, except a datagram that ends
/// its group (`EndOfGroup`).
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub location: Location,
    pub forwarding: ForwardingPreference,
    pub publisher_priority: u8,
    pub status: ObjectStatus,
    pub extension_headers: ExtensionHeaders,
    pub payload: Bytes,
}

/// What the header of a subgroup stream announces (draft-14 §10.4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubgroupStart {
    pub group_id: u64,
    pub subgroup_id: u64,
    pub publisher_priority: u8,
    /// The subgroup holds the last object of its group, so the stream's FIN
    /// ends the group.
    pub ends_group: bool,
}
