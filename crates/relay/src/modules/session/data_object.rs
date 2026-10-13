#[derive(Debug, Clone)]
pub(crate) enum DataObject {
    SubgroupHeader(moqt::object::SubgroupHeader),
    SubgroupObject(moqt::object::SubgroupObjectField),
    ObjectDatagram(moqt::object::ObjectDatagram),
}
