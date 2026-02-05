pub(crate) mod datagram_receiver;
pub(crate) mod stream_receiver;
use std::fmt::Debug;

use crate::modules::core::data_object::DataObject;

#[async_trait::async_trait]
pub(crate) trait DataReceiver: 'static + Send + Sync + Debug {
    fn get_track_alias(&self) -> u64;
    fn datagram(&self) -> bool;
    async fn receive_object(&mut self) -> anyhow::Result<DataObject>;
}
