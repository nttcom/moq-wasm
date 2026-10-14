#[derive(Debug, Clone)]
pub(crate) enum DataObject {
    SubgroupHeader(moqt::object::SubgroupStart),
    SubgroupObject(moqt::object::Object),
    Datagram(moqt::object::Object),
}
