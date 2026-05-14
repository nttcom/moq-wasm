use crate::{TransportProtocol, modules::moqt::data_plane::object::data_object::DataObject};
use std::fmt::Debug;

#[async_trait::async_trait]
pub trait ReceiveStreamType<T: TransportProtocol>: Send + Sync + 'static + Debug {
    fn is_datagram(&self) -> bool;
    async fn receive(&mut self) -> anyhow::Result<DataObject>;
}
