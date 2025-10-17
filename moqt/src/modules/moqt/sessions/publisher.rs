use std::sync::Arc;

use anyhow::bail;

use crate::modules::moqt::{
    enums::ResponseMessage,
    messages::{
        control_message_type::ControlMessageType,
        control_messages::publish_namespace::PublishNamespace,
    },
    protocol::TransportProtocol,
    sessions::inner_session::InnerSession,
    utils,
};

pub struct Publisher<T: TransportProtocol> {
    pub(crate) session: Arc<InnerSession<T>>,
}

impl<T: TransportProtocol> Publisher<T> {
    pub async fn publish_namespace(&self, namespace: String) -> anyhow::Result<()> {
        let vec_namespace = namespace.split('/').map(|s| s.to_string()).collect();
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session.get_request_id();
        self.session
            .sender_map
            .lock()
            .await
            .insert(request_id, sender);
        let publish_namespace = PublishNamespace::new(request_id, vec_namespace, vec![]);
        let bytes =
            utils::create_full_message(ControlMessageType::PublishNamespace, publish_namespace);
        self.session.send_stream.send(&bytes).await?;
        tracing::info!("Publish namespace");
        let result = receiver.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e.to_string())
        }
        let response = result.unwrap();
        match response {
            ResponseMessage::PublishNamespaceOk(response_request_id) => {
                if request_id != response_request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Publish namespace ok");
                    Ok(())
                }
            }
            ResponseMessage::PublishNamespaceError(_, _, _) => {
                tracing::info!("Publish namespace error");
                bail!("Publish namespace error")
            }
            _ => bail!("Protocol violation"),
        }
    }
}
