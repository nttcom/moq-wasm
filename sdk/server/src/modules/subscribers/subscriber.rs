use crate::modules::enums::PublisherEvent;

pub(crate) struct Subscriber<T: moqt::TransportProtocol> {
    pub(crate) id: usize,
    pub(crate) session_id: usize,
    pub(crate) subscriber: moqt::Subscriber<T>,
}

impl<T: moqt::TransportProtocol> Subscriber<T> {
    pub(crate) async fn receive_from_publisher(&mut self) -> anyhow::Result<PublisherEvent> {
        let event = self.subscriber.receive_from_publisher().await?;
        Ok(self.convert(event))
    }

    fn convert(&self, event: moqt::PublisherEvent) -> PublisherEvent {
        match event {
            moqt::PublisherEvent::PublishNameSpace(request_id, items) => {
                PublisherEvent::PublishNameSpace(self.id, request_id, items)
            }
            moqt::PublisherEvent::Publish() => todo!(),
        }
    }
}
