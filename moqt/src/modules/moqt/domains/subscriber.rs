use std::sync::Arc;

use anyhow::bail;

use crate::{
    DataReceiver, DatagramReceiver, StreamDataReceiver, SubscribeOption, Subscription,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{subscribe::Subscribe, subscribe_namespace::SubscribeNamespace},
            },
            enums::ResponseMessage,
            threads::enums::StreamWithObject,
        },
        domains::session_context::SessionContext,
        protocol::TransportProtocol,
    },
};

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
        let subscribe_namespace = SubscribeNamespace::new(request_id, vec_namespace, vec![]);
        self.session
            .send_stream
            .send(
                ControlMessageType::SubscribeNamespace,
                subscribe_namespace.encode(),
            )
            .await?;
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
        option: SubscribeOption,
    ) -> anyhow::Result<Subscription> {
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session.get_request_id();
        self.session
            .sender_map
            .lock()
            .await
            .insert(request_id, sender);
        let subscribe = Subscribe {
            request_id,
            track_namespace: vec_namespace,
            track_name,
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order,
            forward: option.forward,
            filter_type: option.filter_type,
            authorization_tokens: vec![],
            delivery_timeout: None,
        };
        self.session
            .send_stream
            .send(ControlMessageType::Subscribe, subscribe.encode())
            .await?;
        tracing::info!("Subscribe");
        let result = receiver.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e.to_string())
        }
        let response = result.unwrap();
        match response {
            ResponseMessage::SubscribeOk(message) => {
                if request_id != message.request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Subscribe ok");
                    Ok(Subscription::new(message))
                }
            }
            ResponseMessage::SubscribeError(_, _, _) => {
                tracing::info!("Subscribe error");
                bail!("Subscribe error")
            }
            _ => bail!("Protocol violation"),
        }
    }

    pub async fn accept_data_receiver(
        &self,
        subscription: &Subscription,
    ) -> anyhow::Result<DataReceiver<T>> {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<StreamWithObject<T>>();
        self.session
            .notification_map
            .write()
            .await
            .insert(subscription.track_alias, sender);
        let result = receiver
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Failed to receive stream"))?;
        match result {
            StreamWithObject::StreamHeader { stream, header } => {
                let data_receiver = StreamDataReceiver::new(receiver, stream, header).await?;
                Ok(DataReceiver::Stream(data_receiver))
            }
            StreamWithObject::Datagram(object) => {
                let data_receiver = DatagramReceiver::new(object, receiver).await;
                Ok(DataReceiver::Datagram(data_receiver))
            }
        }
    }
}
