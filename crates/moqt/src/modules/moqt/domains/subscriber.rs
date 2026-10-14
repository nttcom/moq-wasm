use std::sync::Arc;

use tracing::Instrument;

use crate::{
    RequestFailure, SubscriberInitiatedSubscription, Subscription,
    fetch::FetchOption,
    modules::moqt::{
        control_plane::{
            control_messages::messages::{
                fetch::Fetch, fetch::FetchParams, fetch_cancel::FetchCancel,
                parameters::authorization_token::AuthorizationToken,
                parameters::content_exists::ContentExists, subscribe::Subscribe,
                subscribe_namespace::SubscribeNamespace, subscribe_update::SubscribeUpdate,
                unsubscribe::Unsubscribe, unsubscribe_namespace::UnsubscribeNamespace,
            },
            enums::ResponseMessage,
        },
        data_plane::stream::{
            fetch_data_receiver::FetchDataReceiver,
            stream_data_receiver_factory::StreamDataReceiverFactory,
        },
        domains::{fetch_handle::FetchHandle, session_context::SessionContext},
        runtime::dispatch::incoming_object::IncomingObject,
        version::{Notification, OutgoingRequest},
    },
    object::{DatagramReceiver, GroupOrder, Location},
    subscribe::{FilterType, SubscribeOption, SubscribeUpdateOption},
    wire::TrackStatusOk,
};

pub enum DataReceiver {
    Stream(StreamDataReceiverFactory),
    Datagram(DatagramReceiver),
}

pub struct Subscriber {
    pub(crate) session: Arc<SessionContext>,
}

impl Subscriber {
    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.subscribe_namespace",
        skip_all,
        fields(namespace = %namespace)
    )]
    pub async fn subscribe_namespace(&self, namespace: String) -> Result<(), RequestFailure> {
        let vec_namespace: Vec<String> = namespace.split('/').map(|s| s.to_string()).collect();
        let request_id = self.session.control.next_request_id();
        let pending = self
            .session
            .control
            .send_request(OutgoingRequest::SubscribeNamespace(
                SubscribeNamespace::new(request_id, vec_namespace, vec![]),
            ))
            .await?;
        tracing::info!("Subscribe namespace");
        let response = pending.response().await?;
        match response {
            ResponseMessage::SubscribeNameSpaceOk(response_request_id) => {
                if request_id != response_request_id {
                    Err(RequestFailure::ProtocolViolation(
                        "response for another request ID",
                    ))
                } else {
                    tracing::info!("Subscribe namespace ok");
                    Ok(())
                }
            }
            ResponseMessage::SubscribeNameSpaceError(request_id, error_code, reason_phrase) => {
                tracing::info!("Subscribe namespace error");
                Err(RequestFailure::Rejected {
                    request_id,
                    code: error_code,
                    reason: reason_phrase,
                })
            }
            _ => Err(RequestFailure::ProtocolViolation(
                "response type does not match the request",
            )),
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
    ) -> Result<Subscription, RequestFailure> {
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let filter_type = option.filter_type;
        let request_id = self.session.control.next_request_id();
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
        let response = self
            .session
            .control
            .send_request(OutgoingRequest::Subscribe(subscribe))
            .await?
            .response()
            .await?;
        match response {
            ResponseMessage::SubscribeOk(message) => {
                if request_id != message.request_id {
                    Err(RequestFailure::ProtocolViolation(
                        "response for another request ID",
                    ))
                } else {
                    tracing::info!("Subscribe ok");
                    if let Err(code) = self
                        .session
                        .register_data_receiver(message.request_id, message.track_alias)
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
                        return Err(RequestFailure::ProtocolViolation(
                            "SUBSCRIBE_OK reused an in-use track alias",
                        ));
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
                            option.subscriber_priority,
                            filter_type,
                        ),
                    ))
                }
            }
            ResponseMessage::SubscribeError(request_id, error_code, reason_phrase) => {
                tracing::info!("Subscribe error");
                Err(RequestFailure::Rejected {
                    request_id,
                    code: error_code,
                    reason: reason_phrase,
                })
            }
            _ => Err(RequestFailure::ProtocolViolation(
                "response type does not match the request",
            )),
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.track_status",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub async fn track_status(
        &self,
        track_namespace: String,
        track_name: String,
        authorization_tokens: Vec<AuthorizationToken>,
    ) -> Result<TrackStatusOk, RequestFailure> {
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let request_id = self.session.control.next_request_id();
        let track_status = Subscribe {
            request_id,
            track_namespace: vec_namespace,
            track_name,
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            forward: false,
            filter_type: FilterType::LargestObject,
            authorization_tokens,
            delivery_timeout: None,
        };
        let response = self
            .session
            .control
            .send_request(OutgoingRequest::TrackStatus(track_status))
            .await?
            .response()
            .await?;
        match response {
            ResponseMessage::TrackStatusOk(message) => {
                if request_id != message.request_id {
                    return Err(RequestFailure::ProtocolViolation(
                        "response for another request ID",
                    ));
                }
                tracing::info!("Track status ok");
                Ok(message)
            }
            ResponseMessage::TrackStatusError(request_id, error_code, reason_phrase) => {
                tracing::info!("Track status error");
                Err(RequestFailure::Rejected {
                    request_id,
                    code: error_code,
                    reason: reason_phrase,
                })
            }
            _ => Err(RequestFailure::ProtocolViolation(
                "response type does not match the request",
            )),
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
    ) -> Result<FetchHandle, RequestFailure> {
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let request_id = self.session.control.next_request_id();

        let pending_fetch = self.session.register_pending_fetch(request_id);

        let fetch = Fetch {
            request_id,
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order,
            fetch_params: FetchParams::Standalone {
                track_namespace: vec_namespace,
                track_name,
                start_location,
                end_location,
            },
            authorization_tokens: vec![],
        };
        let response = self
            .session
            .control
            .send_request(OutgoingRequest::Fetch(fetch))
            .await?
            .response()
            .await?;
        match response {
            ResponseMessage::FetchOk(fetch_ok) => {
                pending_fetch.keep();
                Ok(FetchHandle::new(fetch_ok))
            }
            ResponseMessage::FetchError(request_id, error_code, reason_phrase) => {
                Err(RequestFailure::Rejected {
                    request_id,
                    code: error_code,
                    reason: reason_phrase,
                })
            }
            _ => Err(RequestFailure::ProtocolViolation(
                "response type does not match the request",
            )),
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.fetch_cancel",
        skip_all,
        fields(request_id = %fetch.request_id)
    )]
    pub async fn fetch_cancel(&self, fetch: &FetchHandle) -> anyhow::Result<()> {
        let request_id = fetch.request_id;
        self.session.remove_pending_fetch(request_id);
        self.session
            .control
            .notify(Notification::FetchCancel(FetchCancel::new(request_id)))
            .await?;
        Ok(())
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.fetch_relative_joining",
        skip_all,
        fields(joining_request_id = %subscription.request_id(), joining_start = %joining_start)
    )]
    /// Sends a Relative Joining FETCH for `subscription` (draft-14 §9.16.2):
    /// the range starts at object 0 of the group `joining_start` groups
    /// before the subscription's Largest Location.
    pub async fn fetch_relative_joining(
        &mut self,
        subscription: &Subscription,
        joining_start: u64,
        option: FetchOption,
    ) -> Result<FetchHandle, RequestFailure> {
        let joining_request_id = subscription.request_id();
        let request_id = self.session.control.next_request_id();

        let pending_fetch = self.session.register_pending_fetch(request_id);

        let fetch = Fetch {
            request_id,
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order,
            fetch_params: FetchParams::RelativeJoining {
                joining_request_id,
                joining_start,
            },
            authorization_tokens: vec![],
        };
        let response = self
            .session
            .control
            .send_request(OutgoingRequest::Fetch(fetch))
            .await?
            .response()
            .await?;
        match response {
            ResponseMessage::FetchOk(fetch_ok) => {
                pending_fetch.keep();
                Ok(FetchHandle::new(fetch_ok))
            }
            ResponseMessage::FetchError(request_id, error_code, reason_phrase) => {
                Err(RequestFailure::Rejected {
                    request_id,
                    code: error_code,
                    reason: reason_phrase,
                })
            }
            _ => Err(RequestFailure::ProtocolViolation(
                "response type does not match the request",
            )),
        }
    }

    /// Sends a SUBSCRIBE_UPDATE for `subscription_request_id` (draft-14 §9.10).
    /// The draft defines no response, so this returns once the message is
    /// written.
    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.subscribe_update",
        skip_all,
        fields(subscription_request_id = subscription.request_id(), forward = option.forward)
    )]
    pub async fn subscribe_update(
        &self,
        subscription: &Subscription,
        option: SubscribeUpdateOption,
    ) -> anyhow::Result<()> {
        let subscription_request_id = subscription.request_id();
        let subscribe_update = SubscribeUpdate {
            request_id: self.session.control.next_request_id(),
            subscription_request_id,
            start_location: option.start_location,
            end_group: option.end_group,
            subscriber_priority: option.subscriber_priority,
            forward: option.forward,
            authorization_tokens: vec![],
            delivery_timeout: None,
        };
        self.session
            .control
            .notify(Notification::SubscribeUpdate(subscribe_update))
            .await?;
        Ok(())
    }

    /// Flips Forward and repeats the subscription's current Start Location,
    /// End Group and priority, because a SUBSCRIBE_UPDATE may only narrow a
    /// subscription (draft-14 §9.10).
    pub async fn update_forward(
        &self,
        subscription: &Subscription,
        forward: bool,
    ) -> anyhow::Result<()> {
        let end_group = match subscription.filter_type() {
            FilterType::AbsoluteRange { end_group, .. } => end_group + 1,
            _ => 0,
        };
        self.narrow(subscription, end_group, forward).await
    }

    /// Ends the subscription before `group_id`: groups below it are still
    /// delivered and the publisher answers with PUBLISH_DONE once it has sent
    /// them (draft-14 §9.10, §9.12). The Start Location and priority of the
    /// SUBSCRIBE are repeated, so `forward` has to be the current Forward
    /// State.
    pub async fn end_before_group(
        &self,
        subscription: &Subscription,
        group_id: u64,
        forward: bool,
    ) -> anyhow::Result<()> {
        self.narrow(subscription, group_id, forward).await
    }

    /// `end_group` is the SUBSCRIBE_UPDATE field: the end group plus one, or 0
    /// for an open-ended subscription.
    async fn narrow(
        &self,
        subscription: &Subscription,
        end_group: u64,
        forward: bool,
    ) -> anyhow::Result<()> {
        let largest = match subscription.content_exists() {
            ContentExists::True { location } => Some(location),
            ContentExists::False => None,
        };
        self.subscribe_update(
            subscription,
            SubscribeUpdateOption {
                start_location: subscription.filter_type().start_location(largest),
                end_group,
                subscriber_priority: subscription.subscriber_priority(),
                forward,
            },
        )
        .await
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.unsubscribe",
        skip_all,
        fields(request_id = %subscription.request_id())
    )]
    pub async fn unsubscribe(&self, subscription: &Subscription) -> anyhow::Result<()> {
        let request_id = subscription.request_id();
        self.session.cancel_subscription(request_id).await;
        self.session
            .control
            .notify(Notification::Unsubscribe(Unsubscribe { request_id }))
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
        self.session
            .control
            .notify(Notification::UnsubscribeNamespace(
                UnsubscribeNamespace::new(vec_namespace),
            ))
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
    ) -> anyhow::Result<FetchDataReceiver> {
        let request_id = fetch_handle.request_id;
        let mut fetch_stream_rx = self
            .session
            .fetch_receiver_map
            .lock()
            .expect("fetch_receiver_map poisoned")
            .remove(&request_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "fetch stream receiver already consumed for request_id: {}",
                    request_id
                )
            })?;
        fetch_stream_rx
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Failed to receive fetch stream"))
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
    ) -> anyhow::Result<DataReceiver> {
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
                IncomingObject::Subgroup(_) => {
                    span.record("object_kind", "stream_header");
                }
                IncomingObject::Datagram(_) => {
                    span.record("object_kind", "datagram");
                }
            }
            stream_with_object
        };
        match stream_with_object {
            IncomingObject::Subgroup(first) => Ok(DataReceiver::Stream(
                StreamDataReceiverFactory::new(first, receiver),
            )),
            IncomingObject::Datagram(object) => Ok(DataReceiver::Datagram(DatagramReceiver::new(
                track_alias,
                object,
                receiver,
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use tokio::task::JoinHandle;

    use crate::{
        RequestErrorCode, RequestFailure, Session, SessionEvent,
        fetch::{Fetch, FetchHandle, FetchHandler, FetchOption},
        modules::test_support::{DISCONNECT_TIMEOUT, spawn_connected_dual_sessions, subscription},
        object::{
            ContentExists, ExtensionHeaders, ForwardingPreference, Location, Object, ObjectStatus,
        },
        subscribe::{SubscribeOption, SubscribeUpdateOption},
    };

    const START: Location = Location {
        group_id: 0,
        object_id: 0,
    };
    const END: Location = Location {
        group_id: 1,
        object_id: 0,
    };

    fn fetch_object() -> Object {
        Object {
            location: START,
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: 128,
            status: ObjectStatus::Normal,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from_static(b"object"),
            ends_group: false,
        }
    }

    async fn start_fetch(
        client: &Session,
        server: &Session,
    ) -> (
        JoinHandle<Result<FetchHandle, RequestFailure>>,
        FetchHandler,
    ) {
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .fetch(
                    "ns".to_string(),
                    "track".to_string(),
                    START,
                    END,
                    FetchOption::default(),
                )
                .await
        });
        let SessionEvent::Fetch(handler) = server.receive_event().await.unwrap() else {
            panic!("expected FETCH from the client");
        };
        (request, handler)
    }

    fn has_pending_fetch(client: &Session, request_id: u64) -> bool {
        let session = client.subscriber().session;
        session
            .fetch_notification_map
            .lock()
            .unwrap()
            .contains_key(&request_id)
            || session
                .fetch_receiver_map
                .lock()
                .unwrap()
                .contains_key(&request_id)
    }

    #[tokio::test]
    async fn accepted_fetch_releases_its_stream_sender_and_keeps_delivering_objects() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-fetch-sender").await;
        let (request, handler) = start_fetch(&client, &server).await;
        handler.ok(false, END).await.unwrap();
        let fetch_sender = server
            .publisher()
            .create_fetch_stream(handler.request_id)
            .await
            .unwrap();
        let fetch_handle = request.await.unwrap().unwrap();

        // Act
        let mut fetch_receiver = client
            .subscriber()
            .accept_fetch_receiver(&fetch_handle)
            .await
            .unwrap();
        fetch_sender.send_object(&fetch_object()).await.unwrap();
        let object = fetch_receiver.receive().await.unwrap();

        // Assert
        assert!(
            !client
                .subscriber()
                .session
                .fetch_notification_map
                .lock()
                .unwrap()
                .contains_key(&fetch_handle.request_id)
        );
        assert!(
            matches!(&object, Fetch::Object(received) if *received == fetch_object()),
            "{object:?}"
        );
    }

    #[tokio::test]
    async fn fetch_ok_hands_its_max_cache_duration_to_the_fetch_handle() {
        // Arrange
        let (client, server) =
            spawn_connected_dual_sessions("subscriber-fetch-max-cache-duration").await;
        let (request, mut handler) = start_fetch(&client, &server).await;
        handler.max_cache_duration = Some(2_000);

        // Act
        handler.ok(false, END).await.unwrap();

        // Assert
        let fetch_handle = request.await.unwrap().unwrap();
        assert_eq!(fetch_handle.max_cache_duration, Some(2_000));
    }

    #[tokio::test]
    async fn rejected_fetch_releases_its_pending_state() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-fetch-rejected").await;
        let (request, handler) = start_fetch(&client, &server).await;

        // Act
        handler
            .error(RequestErrorCode::InternalError, "rejected".to_string())
            .await
            .unwrap();
        let result = request.await.unwrap();

        // Assert
        assert!(result.is_err());
        assert!(!has_pending_fetch(&client, handler.request_id));
    }

    #[tokio::test]
    async fn abandoned_fetch_releases_its_pending_state() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-fetch-abandoned").await;
        let (request, handler) = start_fetch(&client, &server).await;

        // Act
        request.abort();
        let _ = request.await;

        // Assert
        assert!(!has_pending_fetch(&client, handler.request_id));
    }

    #[tokio::test]
    async fn update_forward_repeats_the_subscription_fields_with_the_new_forward_flag() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-update-forward").await;
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe(
                    "ns".to_string(),
                    "track".to_string(),
                    SubscribeOption::default(),
                )
                .await
        });
        let SessionEvent::Subscribe(handler) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE from the client");
        };
        let largest = Location {
            group_id: 3,
            object_id: 1,
        };
        handler
            .ok(0, ContentExists::True { location: largest })
            .await
            .unwrap();
        let subscription = request.await.unwrap().unwrap();

        // Act
        client
            .subscriber()
            .update_forward(&subscription, false)
            .await
            .unwrap();

        // Assert
        let SessionEvent::SubscribeUpdate(update) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE_UPDATE from the client");
        };
        assert_eq!(update.subscription_request_id(), handler.request_id());
        assert!(!update.forward());
        assert_eq!(
            update.start_location(),
            Location {
                group_id: 3,
                object_id: 2
            }
        );
        assert_eq!(update.end_group(), 0);
        assert_eq!(update.subscriber_priority(), 128);
    }

    #[tokio::test]
    async fn end_before_group_sends_the_group_as_end_group_plus_one() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-end-before-group").await;
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe(
                    "ns".to_string(),
                    "track".to_string(),
                    SubscribeOption::default(),
                )
                .await
        });
        let SessionEvent::Subscribe(handler) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE from the client");
        };
        let largest = Location {
            group_id: 3,
            object_id: 1,
        };
        handler
            .ok(0, ContentExists::True { location: largest })
            .await
            .unwrap();
        let subscription = request.await.unwrap().unwrap();

        // Act
        client
            .subscriber()
            .end_before_group(&subscription, 7, true)
            .await
            .unwrap();

        // Assert: groups up to 6 stay in the range, which the field encodes as 6 + 1
        let SessionEvent::SubscribeUpdate(update) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE_UPDATE from the client");
        };
        assert_eq!(update.subscription_request_id(), handler.request_id());
        assert_eq!(update.end_group(), 7);
        assert!(update.forward());
        assert_eq!(
            update.start_location(),
            Location {
                group_id: 3,
                object_id: 2
            }
        );
    }

    #[tokio::test]
    async fn subscribe_update_sends_the_given_fields_for_the_given_subscription() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-subscribe-update").await;
        let option = SubscribeUpdateOption {
            start_location: Location {
                group_id: 4,
                object_id: 2,
            },
            end_group: 9,
            subscriber_priority: 7,
            forward: true,
        };

        // Act
        client
            .subscriber()
            .subscribe_update(&subscription(6, 0), option)
            .await
            .unwrap();

        // Assert
        let SessionEvent::SubscribeUpdate(update) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE_UPDATE from the client");
        };
        assert_eq!(update.subscription_request_id(), 6);
        assert_eq!(
            update.start_location(),
            Location {
                group_id: 4,
                object_id: 2
            }
        );
        assert_eq!(update.end_group(), 9);
        assert_eq!(update.subscriber_priority(), 7);
        assert!(update.forward());
    }

    #[tokio::test]
    async fn subscribe_fails_as_soon_as_the_peer_disconnects_without_answering() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-peer-disconnects").await;
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe(
                    "ns".to_string(),
                    "track".to_string(),
                    SubscribeOption::default(),
                )
                .await
        });
        let SessionEvent::Subscribe(_) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE from the client");
        };

        // Act
        drop(server);
        let subscribed = tokio::time::timeout(DISCONNECT_TIMEOUT, request).await;

        // Assert
        assert!(subscribed.unwrap().unwrap().is_err());
    }
}
