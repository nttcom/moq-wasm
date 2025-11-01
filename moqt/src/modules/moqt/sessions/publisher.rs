use std::sync::Arc;

use anyhow::bail;

use crate::modules::moqt::{
    enums::ResponseMessage,
    messages::{
        control_message_type::ControlMessageType,
        control_messages::{publish::Publish, publish_namespace::PublishNamespace},
    },
    options::PublishOption,
    protocol::TransportProtocol,
    sessions::{publication::Publication, session_context::SessionContext},
    utils,
};

pub struct Publisher<T: TransportProtocol> {
    pub(crate) session: Arc<SessionContext<T>>,
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
        tracing::info!("Publish namespace request id: {}", request_id);
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

    pub async fn publish(
        &self,
        track_namespace: String,
        track_name: String,
        option: PublishOption,
    ) -> anyhow::Result<Publication<T>> {
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session.get_request_id();
        self.session
            .sender_map
            .lock()
            .await
            .insert(request_id, sender);
        let publish = Publish {
            request_id,
            track_namespace_tuple: vec_namespace,
            track_name: track_name.clone(),
            track_alias: option.track_alias,
            group_order: option.group_order,
            content_exists: option.content_exists,
            largest_location: option.largest_location,
            forward: option.forward,
            parameters: vec![],
        };
        let bytes = utils::create_full_message(ControlMessageType::Publish, publish);
        self.session.send_stream.send(&bytes).await?;
        tracing::info!("Publish");
        let result = receiver.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e.to_string())
        }
        let response = result.unwrap();
        match response {
            ResponseMessage::PublishOk(message) => {
                if request_id != message.request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Publish ok");
                    Ok(Publication::<T>::new(
                        self.session.clone(),
                        track_namespace,
                        track_name,
                        option.track_alias,
                        message,
                    ))
                }
            }
            ResponseMessage::PublishError(_, _, _) => {
                tracing::info!("Publish error");
                bail!("Publish error")
            }
            _ => bail!("Protocol violation"),
        }
    }
}
