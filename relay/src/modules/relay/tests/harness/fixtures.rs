pub(crate) mod cached_object;
pub(crate) mod data_object;
pub(crate) mod subscription;

pub(crate) fn location(group_id: u64, object_id: u64) -> moqt::Location {
    moqt::Location {
        group_id,
        object_id,
    }
}
