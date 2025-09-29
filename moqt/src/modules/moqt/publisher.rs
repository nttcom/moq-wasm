use std::sync::Arc;

use anyhow::bail;
use bytes::BytesMut;

use crate::{
    Session,
    modules::moqt::{
        control_sender::ControlSender,
        enums::{ReceiveEvent, SubscriberEvent},
        messages::{
            control_messages::{
                publish_namespace::PublishNamespace,
                publish_namespace_error::PublishNamespaceError,
                publish_namespace_ok::PublishNamespaceOk, subscribe::Subscribe,
                subscribe_namespace::SubscribeNamespace,
            },
            moqt_message::MOQTMessage,
            moqt_payload::MOQTPayload,
        },
        protocol::TransportProtocol,
        utils,
    },
};

pub struct Publisher<T: TransportProtocol> {
    pub(crate) session: Arc<Session<T>>,
    pub(crate) shared_send_stream: Arc<tokio::sync::Mutex<ControlSender<T>>>,
    pub(crate) event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
}

impl<T: TransportProtocol> Publisher<T> {
    pub async fn publish_namespace(&self, namespaces: Vec<String>) -> anyhow::Result<()> {
        let request_id = self.session.get_request_id();
        let publish_namespace = PublishNamespace::new(request_id, namespaces, vec![]);
        let bytes = publish_namespace.packetize();
        self.shared_send_stream.lock().await.send(&bytes).await?;
        tokio::select! {
            publish_namespace_ok = utils::start_receive::<PublishNamespaceOk>(self.event_sender.subscribe()) => {
                let publish_namespace_ok = publish_namespace_ok?;
                if request_id == publish_namespace_ok.request_id {
                    Ok(())
                } else {
                    bail!("unmatched request id")
                }
            }
            publish_namespace_error = utils::start_receive::<PublishNamespaceError>(self.event_sender.subscribe()) => {
                let publish_namespace_error = publish_namespace_error?;
                if request_id == publish_namespace_error.request_id {
                    Ok(())
                } else {
                    bail!("unmatched request id")
                }
            }
        }
    }

    pub async fn receive_from_subscriber(&mut self) -> anyhow::Result<SubscriberEvent> {
        let mut receiver = self.event_sender.subscribe();
        let receive_message = receiver.recv().await?;
        match receive_message {
            ReceiveEvent::Message(binary) => self.resolve_message(binary),
            ReceiveEvent::Error() => bail!("Error occurred."),
        }
    }

    fn resolve_message(&self, binary_message: Vec<u8>) -> anyhow::Result<SubscriberEvent> {
        // subscribe_namespace
        // subscribe
        let mut bytes_mut = BytesMut::from(binary_message.as_slice());
        if let Ok(subscribe_namespace) = SubscribeNamespace::depacketize(&mut bytes_mut) {
            Ok(SubscriberEvent::SubscribeNameSpace(
                subscribe_namespace.request_id(),
                subscribe_namespace.track_namespace_prefix(),
            ))
        } else if let Ok(subscribe) = Subscribe::depacketize(&mut bytes_mut) {
            todo!()
        } else {
            bail!("message unmatches.")
        }
    }
}
