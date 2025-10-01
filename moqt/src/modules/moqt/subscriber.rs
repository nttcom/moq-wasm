use std::sync::Arc;

use anyhow::bail;

use crate::modules::moqt::{
        control_sender::ControlSender, enums::ReceiveEvent, messages::{
            control_messages::{
                subscribe_namespace::SubscribeNamespace,
                subscribe_namespace_error::SubscribeNamespaceError,
                subscribe_namespace_ok::SubscribeNamespaceOk,
            },
            moqt_message::MOQTMessage,
        }, protocol::TransportProtocol, sessions::inner_session::InnerSession, utils
    };

pub struct Subscriber<T: TransportProtocol> {
    pub(crate) session: Arc<InnerSession<T>>,
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
}
