#[derive(Debug, Clone)]
pub(crate) enum DataObject {
    SubgroupHeader(moqt::object::SubgroupStart),
    SubgroupObject(moqt::object::Object),
    /// A subgroup object whose `payload_len` payload bytes follow as
    /// `SubgroupPayload` pieces; `object.payload` is empty.
    SubgroupObjectStart {
        object: moqt::object::Object,
        payload_len: usize,
    },
    /// The next bytes of the current object's payload; `last` ends it.
    SubgroupPayload {
        data: bytes::Bytes,
        last: bool,
    },
    Datagram(moqt::object::Object),
}
