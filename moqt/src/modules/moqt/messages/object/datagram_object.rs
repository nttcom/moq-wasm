use crate::modules::moqt::messages::{
    data_streams::extension_header::ExtensionHeader, object::object_status::ObjectStatus,
};

pub struct DatagramObject {
    pub(crate) message_type: u64,
    pub(crate) track_alias: u64,
    pub(crate) group_id: u64,
    pub(crate) object_id: Option<u64>,
    pub(crate) publisher_priority: u8,
    pub(crate) extension_headers: Vec<ExtensionHeader>,
    pub(crate) object_status: Option<ObjectStatus>,
    pub(crate) object_payload: Vec<u8>,
}
