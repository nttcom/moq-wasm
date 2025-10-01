use std::sync::Arc;

use anyhow::bail;

use crate::modules::moqt::{
        control_sender::ControlSender, enums::ReceiveEvent, messages::{
            control_messages::{
                publish_namespace::PublishNamespace,
                publish_namespace_error::PublishNamespaceError,
                publish_namespace_ok::PublishNamespaceOk,
            },
            moqt_message::MOQTMessage,
        }, protocol::TransportProtocol, sessions::inner_session::InnerSession, utils
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
}
