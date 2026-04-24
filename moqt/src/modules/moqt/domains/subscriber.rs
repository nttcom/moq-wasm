use std::sync::Arc;

use anyhow::bail;

use crate::{
    DatagramReceiver, SubscribeOption, Subscription,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{subscribe::Subscribe, subscribe_namespace::SubscribeNamespace},
            },
            enums::ResponseMessage,
            threads::enums::StreamWithObject,
        },
        data_plane::streams::stream::{
            stream_data_receiver::StreamDataReceiver,
            stream_data_receiver_factory::StreamDataReceiverFactory,
        },
        domains::session_context::SessionContext,
        protocol::TransportProtocol,
    },
};

pub enum DataReceiver<T: TransportProtocol> {
    Stream(StreamDataReceiverFactory<T>),
    Datagram(DatagramReceiver<T>),
}

pub struct Subscriber<T: TransportProtocol> {
    pub(crate) session: Arc<SessionContext<T>>,
}

impl<T: TransportProtocol> Subscriber<T> {
    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.subscribe_namespace",
        skip_all,
        fields(namespace = %namespace)
    )]
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
            bail!("Failed to receive message: {}", e)
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

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.subscribe",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name, subscriber_priority = option.subscriber_priority, forward = option.forward)
    )]
    pub async fn subscribe(
        &mut self,
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
        let result = receiver.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e)
        }
        let response = result.unwrap();
        match response {
            ResponseMessage::SubscribeOk(message) => {
                if request_id != message.request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Subscribe ok");
                    let (sender, receiver) =
                        tokio::sync::mpsc::unbounded_channel::<StreamWithObject<T>>();
                    self.session
                        .notification_map
                        .write()
                        .await
                        .insert(message.track_alias, sender);
                    self.session
                        .receiver_map
                        .lock()
                        .await
                        .insert(message.track_alias, receiver);
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

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.accept_data_receiver",
        skip_all,
        fields(track_alias = subscription.track_alias)
    )]
    pub async fn accept_data_receiver(
        &mut self,
        subscription: &Subscription,
    ) -> anyhow::Result<DataReceiver<T>> {
        let track_alias = subscription.track_alias;
        let stream_with_object = self
            .session
            .receiver_map
            .lock()
            .await
            .get_mut(&track_alias)
            .ok_or_else(|| anyhow::anyhow!("No receiver for track_alias: {}", track_alias))?
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Failed to receive stream"))?;
        match stream_with_object {
            StreamWithObject::StreamHeader { stream, header } => {
                let first = StreamDataReceiver::new(stream, header).await?;
                let rest = self
                    .session
                    .receiver_map
                    .lock()
                    .await
                    .remove(&track_alias)
                    .ok_or_else(|| {
                        anyhow::anyhow!("receiver already removed for track_alias: {}", track_alias)
                    })?;
                Ok(DataReceiver::Stream(StreamDataReceiverFactory::new(
                    first, rest,
                )))
            }
            StreamWithObject::Datagram(object) => {
                // Datagram は1回限りなので map から除去する
                let receiver = self
                    .session
                    .receiver_map
                    .lock()
                    .await
                    .remove(&track_alias)
                    .unwrap();
                let data_receiver = DatagramReceiver::new(object, receiver).await;
                Ok(DataReceiver::Datagram(data_receiver))
            }
        }
    }
}
