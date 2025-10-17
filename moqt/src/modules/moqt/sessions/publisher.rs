use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use anyhow::bail;

use crate::{
    Forward, SubscriberPriority,
    modules::moqt::{
        enums::ResponseMessage,
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                enums::FilterType, group_order::GroupOrder, publish::Publish,
                publish_namespace::PublishNamespace,
            },
        },
        options::PublishOption,
        protocol::TransportProtocol,
        sessions::inner_session::InnerSession,
        utils,
    },
};

pub struct PublishResult {
    pub group_order: GroupOrder,
    pub subscriber_priority: SubscriberPriority,
    pub forward: Forward,
    pub filter_type: FilterType,
    pub largest_location_group_id: Option<u64>,
    pub largest_location_object_id: Option<u64>,
    pub end_group: Option<u64>,
}

pub struct Publisher<T: TransportProtocol> {
    session: Arc<InnerSession<T>>,
    track_alias: AtomicU64,
}

impl<T: TransportProtocol> Publisher<T> {
    pub(crate) fn new(session: Arc<InnerSession<T>>) -> Self {
        Self {
            session,
            track_alias: AtomicU64::new(0),
        }
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

    pub async fn publish(
        &self,
        track_namespace: String,
        track_name: String,
        option: PublishOption,
    ) -> anyhow::Result<PublishResult> {
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
            track_name,
            track_alias: self.get_track_alias(),
            group_order: option.group_order,
            content_exists: option.content_exists,
            largest_location: option.largest_location,
            forward: option.forward,
            parameters: vec![],
        };
        let bytes = utils::create_full_message(ControlMessageType::PublishNamespace, publish);
        self.session.send_stream.send(&bytes).await?;
        tracing::info!("Publish namespace");
        let result = receiver.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e.to_string())
        }
        let response = result.unwrap();
        match response {
            ResponseMessage::PublishOk(
                response_request_id,
                group_order,
                subscriber_priority,
                forward,
                filter_type,
                option_location,
                option_end_group,
            ) => {
                if request_id != response_request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Publish ok");
                    let (largest_location_group_id, largest_location_object_id) =
                        if option_location.is_some() {
                            let location = option_location.unwrap();
                            (Some(location.group_id), Some(location.object_id))
                        } else {
                            (None, None)
                        };
                    Ok(PublishResult {
                        group_order,
                        subscriber_priority,
                        forward,
                        filter_type,
                        largest_location_group_id,
                        largest_location_object_id,
                        end_group: option_end_group,
                    })
                }
            }
            ResponseMessage::PublishError(_, _, _) => {
                tracing::info!("Publish error");
                bail!("Publish error")
            }
            _ => bail!("Protocol violation"),
        }
    }

    fn get_track_alias(&self) -> u64 {
        let id = self.track_alias.load(Ordering::SeqCst);
        tracing::debug!("request_id: {}", id);
        self.track_alias.fetch_add(1, Ordering::SeqCst);
        id
    }
}
