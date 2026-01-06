use crate::modules::moqt::data_plane::{
    object::data_object::DataObject, streams::stream_type::SendStreamType,
};

pub struct DataSender<S: SendStreamType> {
    send_stream: S,
}

impl<S: SendStreamType> DataSender<S> {
    pub fn new(send_stream: S) -> Self {
        Self { send_stream }
    }

    pub fn is_datagram(&self) -> bool {
        self.send_stream.is_datagram()
    }

    pub async fn send(&mut self, data: DataObject) -> anyhow::Result<()> {
        self.send_stream.send(data).await
    }
}
