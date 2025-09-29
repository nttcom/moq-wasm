pub(crate) struct Publisher<T: moqt::TransportProtocol> {
    pub(crate) id: usize,
    pub(crate) session_id: usize,
    pub(crate) publisher: moqt::Publisher<T>
}

impl<T: moqt::TransportProtocol> Publisher<T> {
    pub(crate) async fn receive_from_subscriber(&mut self) {
        let message = self.publisher.receive_from_subscriber().await;
        
    }
}

