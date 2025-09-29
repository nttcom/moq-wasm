use std::sync::Arc;

use anyhow::bail;
use bytes::BytesMut;

use crate::{
    modules::moqt::{
        control_sender::ControlSender,
        enums::{PublisherEvent, ReceiveEvent},
        messages::{
            control_messages::{
                publish_namespace::PublishNamespace, subscribe_namespace::SubscribeNamespace,
                subscribe_namespace_error::SubscribeNamespaceError,
                subscribe_namespace_ok::SubscribeNamespaceOk,
            }, moqt_message::MOQTMessage
        },
        protocol::TransportProtocol, utils,
    }, Session
};

pub struct Subscriber<T: TransportProtocol> {
    pub(crate) session: Arc<Session<T>>,
    pub(crate) shared_send_stream: Arc<tokio::sync::Mutex<ControlSender<T>>>,
    pub(crate) event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
}

impl<T: TransportProtocol> Subscriber<T> {
    pub async fn subscribe_namespace(&self, namespaces: Vec<String>) -> anyhow::Result<()> {
        let request_id = self.session.get_request_id();
        let subscribe_namespace = SubscribeNamespace::new(request_id, namespaces, vec![]);
        let bytes = subscribe_namespace.packetize();
        self.shared_send_stream.lock().await.send(&bytes).await?;
        tokio::select! {
            _ = utils::start_receive::<SubscribeNamespaceOk>(self.event_sender.subscribe()) => {
                Ok(())
            }
            _ = utils::start_receive::<SubscribeNamespaceError>(self.event_sender.subscribe()) => {
                bail!("Error occurred.")
            }
        }
    }

    pub fn subscribe() {}

    pub async fn receive_from_publisher(&mut self) -> anyhow::Result<PublisherEvent> {
        let mut receiver = self.event_sender.subscribe();
        let receive_message = receiver.recv().await?;
        match receive_message {
            ReceiveEvent::Message(binary) => self.resolve_message(binary),
            ReceiveEvent::Error() => bail!("Error occurred."),
        }
    }

    fn resolve_message(&self, binary_message: Vec<u8>) -> anyhow::Result<PublisherEvent> {
        // publish_namespace
        // publish
        let mut bytes_mut = BytesMut::from(binary_message.as_slice());
        if let Ok(publish_namespace) = PublishNamespace::depacketize(&mut bytes_mut) {
            Ok(PublisherEvent::PublishNameSpace(
                publish_namespace.request_id,
                publish_namespace.track_namespace,
            ))
        } else {
            bail!("message unmatches.")
        }
    }
}
