pub(crate) struct Subscriber<T: moqt::TransportProtocol> {
    pub(crate) id: usize,
    pub(crate) session_id: usize,
    pub(crate) subscriber: moqt::Subscriber<T>,
}

impl<T: moqt::TransportProtocol> Subscriber<T> {
    pub(crate) async fn receive_from_publisher(&mut self) {
        self.subscriber.receive_from_publisher().await;
    }
}
