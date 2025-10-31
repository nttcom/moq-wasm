use async_trait::async_trait;

use crate::modules::core::datagram_sender::DatagramSender;

#[async_trait]
pub(crate) trait Publication {
    async fn create_stream(&self) -> anyhow::Result<()>;
    fn create_datagram(&self) -> Box<dyn DatagramSender>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Publication for moqt::Publication<T> {
    async fn create_stream(&self) -> anyhow::Result<()> {
        todo!()
    }

    fn create_datagram(&self) -> Box<dyn DatagramSender> {
        let sender = self.create_datagram();
        Box::new(sender)
    }
}
