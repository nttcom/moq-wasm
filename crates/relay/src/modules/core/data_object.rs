#[derive(Debug, Clone)]
pub(crate) enum DataObject {
    SubgroupHeader(moqt::SubgroupHeader),
    SubgroupObject(moqt::SubgroupObjectField),
    ObjectDatagram(moqt::ObjectDatagram),
}
