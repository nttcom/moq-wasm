use std::sync::Arc;

use anyhow::bail;

use crate::{
    RequestId,
    modules::moqt::{
        control_sender::ControlSender,
        enums::ReceiveEvent,
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                namespace_ok::NamespaceOk, publish_namespace::PublishNamespace,
                request_error::RequestError,
            },
            moqt_message::MOQTMessage,
        },
        protocol::TransportProtocol,
        sessions::inner_session::InnerSession,
        utils,
    },
};

pub struct Publisher<T: TransportProtocol> {
    pub(crate) session: Arc<InnerSession<T>>,
    pub(crate) shared_send_stream: Arc<tokio::sync::Mutex<ControlSender<T>>>,
    pub(crate) event_sender: tokio::sync::broadcast::Sender<ReceiveEvent>,
}

impl<T: TransportProtocol> Publisher<T> {
    pub async fn publish_namespace(&self, namespaces: Vec<String>) -> anyhow::Result<()> {
        let request_id = self.session.get_request_id();
        let publish_namespace = PublishNamespace::new(request_id, namespaces, vec![]);
        let bytes = publish_namespace.packetize();
        let bytes = utils::add_message_type(ControlMessageType::PublishNamespace, bytes);
        self.shared_send_stream.lock().await.send(&bytes).await?;
        tokio::select! {
            publish_namespace_ok = utils::start_receive::<NamespaceOk>(ControlMessageType::PublishNamespaceOk, self.event_sender.subscribe()) => {
                let publish_namespace_ok = publish_namespace_ok?;
                if request_id == publish_namespace_ok.request_id {
                    Ok(())
                } else {
                    bail!("unmatched request id")
                }
            }
            publish_namespace_error = utils::start_receive::<RequestError>(ControlMessageType::PublishNamespaceError, self.event_sender.subscribe()) => {
                let publish_namespace_error = publish_namespace_error?;
                if request_id == publish_namespace_error.request_id {
                    Ok(())
                } else {
                    bail!("Error occured: Publish Namespace")
                }
            }
        }
    }

    pub async fn subscribe_namespace_ok(&self, request_id: RequestId) -> anyhow::Result<()> {
        let namespace_ok = NamespaceOk { request_id };
        let bytes = namespace_ok.packetize();
        let bytes = utils::add_message_type(ControlMessageType::SubscribeNamespaceOk, bytes);
        self.shared_send_stream.lock().await.send(&bytes).await
    }

    pub async fn subscribe_namespace_error(
        &self,
        request_id: RequestId,
        error_code: u64,
        reason_phrase: String,
    ) -> anyhow::Result<()> {
        let request_error = RequestError {
            request_id,
            error_code,
            reason_phrase,
        };
        let bytes = request_error.packetize();
        let bytes = utils::add_message_type(ControlMessageType::SubscribeNamespaceError, bytes);
        self.shared_send_stream.lock().await.send(&bytes).await
    }
}
