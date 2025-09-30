use crate::modules::enums::SubscriberEvent;

pub(crate) struct Publisher<T: moqt::TransportProtocol> {
    pub(crate) id: usize,
    pub(crate) session_id: usize,
    pub(crate) publisher: moqt::Publisher<T>,
}

impl<T: moqt::TransportProtocol> Publisher<T> {
    pub(crate) async fn receive_from_subscriber(&mut self) -> anyhow::Result<SubscriberEvent> {
        let event = self.publisher.receive_from_subscriber().await?;
        Ok(self.convert(event))
    }

    fn convert(&self, event: moqt::SubscriberEvent) -> SubscriberEvent {
        match event {
            moqt::SubscriberEvent::SubscribeNameSpace(request_id, items) => {
                SubscriberEvent::SubscribeNameSpace(self.id, request_id, items)
            }
            moqt::SubscriberEvent::Subscribe() => todo!(),
        }
    }
}
