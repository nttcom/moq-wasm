use std::sync::Arc;

use anyhow::bail;

use crate::modules::moqt::{
    enums::ResponseMessage,
    messages::{
        control_message_type::ControlMessageType,
        control_messages::subscribe_namespace::SubscribeNamespace,
    },
    protocol::TransportProtocol,
    sessions::inner_session::InnerSession,
    utils,
};

pub struct Subscriber<T: TransportProtocol> {
    pub(crate) session: Arc<InnerSession<T>>,
}

impl<T: TransportProtocol> Subscriber<T> {
    pub async fn subscribe_namespace(&self, namespaces: Vec<String>) -> anyhow::Result<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session.get_request_id();
        self.session
            .sender_map
            .lock()
            .await
            .insert(request_id, sender);
        let publish_namespace = SubscribeNamespace::new(request_id, namespaces, vec![]);
        let bytes =
            utils::create_full_message(ControlMessageType::SubscribeNamespace, publish_namespace);
        self.session.send_stream.send(&bytes).await?;
        tracing::info!("Subscribe namespace");
        let result = receiver.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e.to_string())
        }
        let response = result.unwrap();
        match response {
            ResponseMessage::SubscribeNameSpaceOk(response_request_id) => {
                if request_id != response_request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Subscribe namespace ok");
                    Ok(())
                }
            }
            ResponseMessage::SubscribeNameSpaceError(_, _, _) => {
                tracing::info!("Subscribe namespace error");
                bail!("Subscribe namespace error")
            }
            _ => bail!("Protocol violation"),
        }
    }
}
