use std::sync::Arc;

use anyhow::bail;

use crate::modules::{
    moqt::{
        enums::ResponseMessage,
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                group_order::GroupOrder, subscribe::Subscribe,
                subscribe_namespace::SubscribeNamespace,
            },
        },
        options::SubscribeOption,
        protocol::TransportProtocol,
        sessions::session_context::SessionContext,
        streams::{
            datagram::datagram_receiver::DatagramReceiver, stream::stream_receiver::StreamReceiver,
        },
        utils,
    },
    transport::transport_connection::TransportConnection,
};

pub struct SubscribeResult {
    pub track_alias: u64,
    pub expires: u64,
    pub group_order: GroupOrder,
    pub content_exists: bool,
    pub start_location_group_id: Option<u64>,
    pub start_location_object_id: Option<u64>,
}

pub struct Subscriber<T: TransportProtocol> {
    pub(crate) session: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> Subscriber<T> {
    pub async fn subscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        let vec_namespace = namespace.split('/').map(|s| s.to_string()).collect();
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session.get_request_id();
        self.session
            .sender_map
            .lock()
            .await
            .insert(request_id, sender);
        let publish_namespace = SubscribeNamespace::new(request_id, vec_namespace, vec![]);
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

    pub async fn subscribe(
        &self,
        track_namespace: String,
        track_name: String,
        track_alias: u64,
        option: SubscribeOption,
    ) -> anyhow::Result<SubscribeResult> {
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session.get_request_id();
        self.session
            .sender_map
            .lock()
            .await
            .insert(request_id, sender);
        let publish_namespace = Subscribe {
            request_id,
            track_alias,
            track_namespace: vec_namespace,
            track_name,
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order,
            forward: option.forward,
            filter_type: option.filter_type,
            start_location: option.start_location,
            end_group: option.end_group,
            subscribe_parameters: vec![],
        };
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
            ResponseMessage::SubscribeOk(
                response_request_id,
                track_alias,
                expires,
                group_order,
                content_exists,
                option_location,
            ) => {
                if request_id != response_request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Subscribe namespace ok");
                    let (start_location_group_id, start_location_object_id) =
                        if option_location.is_some() {
                            let location = option_location.unwrap();
                            (Some(location.group_id), Some(location.object_id))
                        } else {
                            (None, None)
                        };
                    Ok(SubscribeResult {
                        track_alias,
                        expires,
                        group_order,
                        content_exists,
                        start_location_group_id,
                        start_location_object_id,
                    })
                }
            }
            ResponseMessage::SubscribeError(_, _, _) => {
                tracing::info!("Subscribe namespace error");
                bail!("Subscribe namespace error")
            }
            _ => bail!("Protocol violation"),
        }
    }

    pub async fn accept_stream(&self) -> anyhow::Result<StreamReceiver<T>> {
        let send_stream = self.session.transport_connection.accept_uni().await?;
        Ok(StreamReceiver::new(send_stream))
    }

    pub async fn accept_datagram(&self) -> DatagramReceiver<T> {
        DatagramReceiver {
            session_context: self.session.clone(),
        }
    }
}
