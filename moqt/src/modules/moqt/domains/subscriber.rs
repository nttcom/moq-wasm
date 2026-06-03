use std::sync::Arc;

use anyhow::bail;
use tracing::Instrument;

use crate::{
    DatagramReceiver, FetchOption, Location, SubscribeOption, SubscriberInitiatedSubscription,
    Subscription,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    fetch::Fetch, fetch::FetchType, subscribe::Subscribe,
                    subscribe_namespace::SubscribeNamespace, unsubscribe::Unsubscribe,
                    unsubscribe_namespace::UnsubscribeNamespace,
                },
            },
            enums::ResponseMessage,
        },
        data_plane::stream::{
            fetch_data_receiver::FetchDataReceiver, stream_data_receiver::StreamDataReceiver,
            stream_data_receiver_factory::StreamDataReceiverFactory,
        },
        domains::{fetch_handle::FetchHandle, session_context::SessionContext},
        protocol::TransportProtocol,
        runtime::dispatch::incoming_object::IncomingObject,
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
        let filter_type = option.filter_type;
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
            track_name: track_name.clone(),
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
                    if let Err(code) = self
                        .session
                        .register_data_receiver(message.track_alias)
                        .await
                    {
                        // The track alias is already bound to another active subscription.
                        // draft-14 §9.8: the subscriber MUST close with DUPLICATE_TRACK_ALIAS.
                        tracing::error!(
                            track_alias = message.track_alias,
                            "SUBSCRIBE_OK reused an in-use track alias; closing session"
                        );
                        self.session
                            .close_with_error(code, "SUBSCRIBE_OK reused an in-use track alias");
                        bail!("Duplicate track alias")
                    }
                    tracing::info!(
                        track_alias = message.track_alias,
                        "subscriber registered incoming object receiver after SUBSCRIBE_OK"
                    );
                    Ok(Subscription::SubscriberInitiated(
                        SubscriberInitiatedSubscription::new(
                            track_namespace,
                            track_name,
                            message,
                            filter_type,
                        ),
                    ))
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
        name = "moqt.subscriber.fetch",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub async fn fetch(
        &mut self,
        track_namespace: String,
        track_name: String,
        start_location: Location,
        end_location: Location,
        option: FetchOption,
    ) -> anyhow::Result<FetchHandle> {
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let request_id = self.session.get_request_id();

        let (fetch_stream_tx, fetch_stream_rx) =
            tokio::sync::mpsc::unbounded_channel::<IncomingObject<T>>();
        self.session
            .fetch_notification_map
            .write()
            .await
            .insert(request_id, fetch_stream_tx);
        self.session
            .fetch_receiver_map
            .lock()
            .await
            .insert(request_id, fetch_stream_rx);

        let (fetch_message_tx, fetch_message_rx) =
            tokio::sync::oneshot::channel::<ResponseMessage>();
        self.session
            .sender_map
            .lock()
            .await
            .insert(request_id, fetch_message_tx);
        let fetch = Fetch {
            request_id,
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order,
            fetch_type: FetchType::Standalone {
                track_namespace: vec_namespace,
                track_name,
                start_location,
                end_location,
            },
            authorization_tokens: vec![],
        };
        self.session
            .send_stream
            .send(ControlMessageType::Fetch, fetch.encode())
            .await?;
        let result = fetch_message_rx.await;
        if let Err(e) = result {
            bail!("Failed to receive message: {}", e)
        }
        let response = result.unwrap();
        match response {
            ResponseMessage::FetchOk(fetch_ok) => Ok(FetchHandle::new(fetch_ok)),
            ResponseMessage::FetchError(_, _, _) => {
                self.session
                    .fetch_notification_map
                    .write()
                    .await
                    .remove(&request_id);
                self.session
                    .fetch_receiver_map
                    .lock()
                    .await
                    .remove(&request_id);
                bail!("Fetch error")
            }
            _ => bail!("Protocol violation"),
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.unsubscribe",
        skip_all,
        fields(subscribe_id = %subscribe_id)
    )]
    pub async fn unsubscribe(&self, subscribe_id: u64) -> anyhow::Result<()> {
        let unsubscribe = Unsubscribe {
            request_id: subscribe_id,
        };
        self.session
            .send_stream
            .send(ControlMessageType::UnSubscribe, unsubscribe.encode())
            .await?;
        Ok(())
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.unsubscribe_namespace",
        skip_all,
        fields(namespace = %namespace)
    )]
    pub async fn unsubscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        let vec_namespace = namespace.split('/').map(|s| s.to_string()).collect();
        let unsubscribe_namespace = UnsubscribeNamespace::new(vec_namespace);
        self.session
            .send_stream
            .send(
                ControlMessageType::UnSubscribeNamespace,
                unsubscribe_namespace.encode(),
            )
            .await?;
        Ok(())
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.accept_fetch_receiver",
        skip_all,
        fields(request_id = fetch_handle.request_id)
    )]
    pub async fn accept_fetch_receiver(
        &mut self,
        fetch_handle: &FetchHandle,
    ) -> anyhow::Result<FetchDataReceiver<T>> {
        let request_id = fetch_handle.request_id;
        let mut fetch_stream_rx = self
            .session
            .fetch_receiver_map
            .lock()
            .await
            .remove(&request_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "fetch stream receiver already consumed for request_id: {}",
                    request_id
                )
            })?;
        let incoming_track_data = fetch_stream_rx
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Failed to receive fetch stream"))?;
        match incoming_track_data {
            IncomingObject::Fetch { stream, header } => Ok(FetchDataReceiver::new(stream, header)),
            _ => unreachable!("FetchReceiver can only receive IncomingObject::Fetch"),
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.accept_data_receiver",
        skip_all,
        fields(track_alias = subscription.track_alias())
    )]
    pub async fn accept_data_receiver(
        &mut self,
        subscription: &Subscription,
    ) -> anyhow::Result<DataReceiver<T>> {
        let track_alias = subscription.track_alias();
        let mut receiver = self
            .session
            .receiver_map
            .lock()
            .await
            .remove(&track_alias)
            .ok_or_else(|| anyhow::anyhow!("No receiver for track_alias: {}", track_alias))?;
        let stream_with_object = {
            let span = tracing::info_span!(
                "moqt.subscriber.wait_first_object",
                track_alias = track_alias,
                object_kind = tracing::field::Empty,
            );
            let stream_with_object = async {
                receiver
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("Failed to receive stream"))
            }
            .instrument(span.clone())
            .await?;
            match stream_with_object {
                IncomingObject::StreamHeader { .. } => {
                    span.record("object_kind", "stream_header");
                }
                IncomingObject::Datagram(_) => {
                    span.record("object_kind", "datagram");
                }
                IncomingObject::Fetch { .. } => {
                    span.record("object_kind", "fetch");
                }
            }
            stream_with_object
        };
        match stream_with_object {
            IncomingObject::StreamHeader { stream, header } => {
                let first = StreamDataReceiver::new(stream, header).await?;
                Ok(DataReceiver::Stream(StreamDataReceiverFactory::new(
                    first, receiver,
                )))
            }
            IncomingObject::Datagram(object) => {
                let data_receiver = DatagramReceiver::new(object, receiver).await;
                Ok(DataReceiver::Datagram(data_receiver))
            }
            IncomingObject::Fetch { .. } => {
                anyhow::bail!("Expected StreamHeader or Datagram but got Fetch")
            }
        }
    }
}
