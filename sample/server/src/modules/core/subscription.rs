use async_trait::async_trait;

use crate::modules::core::datagram_receiver::DatagramReceiver;

pub(crate) enum Acceptance {
    Datagram(Box<dyn DatagramReceiver>, moqt::DatagramObject),
}

#[async_trait]
pub trait Subscription {
    async fn accept_stream_or_datagram(&self, track_alias: u64) -> anyhow::Result<Acceptance>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Subscription for moqt::Subscription<T> {
    async fn accept_stream_or_datagram(&self, track_alias: u64) -> anyhow::Result<Acceptance> {
        let result = self.accept_stream_or_datagram(track_alias).await?;
        match result {
            moqt::Acceptance::Stream(stream) => todo!(),
            moqt::Acceptance::Datagram(datagram, object) => {
                Ok(Acceptance::Datagram(Box::new(datagram), object))
            }
        }
    }
}
