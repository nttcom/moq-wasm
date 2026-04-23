use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::bail;

use crate::{
    DatagramSender,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{publish::Publish, publish_namespace::PublishNamespace},
            },
            enums::ResponseMessage,
            options::PublishOption,
        },
        data_plane::stream::data_sender_factory::StreamDataSenderFactory,
        domains::{published_resource::PublishedResource, session_context::SessionContext},
        protocol::TransportProtocol,
    },
};

pub struct Publisher<T: TransportProtocol> {
    pub(crate) session: Arc<SessionContext<T>>,
}

static NEXT_TRACK_ALIAS: AtomicU64 = AtomicU64::new(0);

impl<T: TransportProtocol> Publisher<T> {
    fn next_track_alias() -> u64 {
        NEXT_TRACK_ALIAS.fetch_add(1, Ordering::SeqCst)
    }

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
        self.session
            .send_stream
            .send(
                ControlMessageType::PublishNamespace,
                publish_namespace.encode(),
            )
            .await?;
        tracing::info!("Publish namespace request id: {}", request_id);
        let result = receiver.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e)
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
    ) -> anyhow::Result<PublishedResource> {
        let track_alias = Self::next_track_alias();
        tracing::debug!("track alias: {}", track_alias);
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
            track_alias,
            group_order: option.group_order,
            content_exists: option.content_exists,
            forward: option.forward,
            authorization_tokens: vec![],
            delivery_timeout: None,
            max_duration: None,
        };
        let bytes = publish.encode();
        self.session
            .send_stream
            .send(ControlMessageType::Publish, bytes)
            .await?;
        tracing::info!("Publish");
        let result = receiver.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e)
        }
        let response = result.unwrap();
        match response {
            ResponseMessage::PublishOk(message) => {
                if request_id != message.request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Publish ok");
                    Ok(PublishedResource::new(
                        track_namespace,
                        track_name,
                        track_alias,
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

    pub fn create_stream(
        &self,
        published_resource: &PublishedResource,
    ) -> StreamDataSenderFactory<T> {
        StreamDataSenderFactory::new(published_resource.track_alias, self.session.clone())
    }

    pub fn create_datagram(&self, published_resource: &PublishedResource) -> DatagramSender<T> {
        DatagramSender::new(published_resource.track_alias, self.session.clone())
    }
}
