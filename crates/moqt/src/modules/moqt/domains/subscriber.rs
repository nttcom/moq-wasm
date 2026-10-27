use std::sync::Arc;

use tracing::Instrument;

use crate::{
    RequestFailure, SubscriberInitiatedSubscription, Subscription, TrackReader,
    fetch::FetchOption,
    modules::moqt::{
        control_plane::{
            control_messages::messages::{
                fetch::Fetch, fetch::FetchParams, fetch_cancel::FetchCancel, fetch_ok::FetchOk,
                parameters::authorization_token::AuthorizationToken,
                parameters::content_exists::ContentExists, subscribe::Subscribe,
                subscribe_namespace::SubscribeNamespace, subscribe_ok::SubscribeOk,
                subscribe_update::SubscribeUpdate, unsubscribe::Unsubscribe,
                unsubscribe_namespace::UnsubscribeNamespace,
            },
            enums::ResponseMessage,
        },
        data_plane::stream::{
            fetch_data_receiver::FetchDataReceiver,
            stream_data_receiver_factory::StreamDataReceiverFactory,
        },
        domains::{
            connection::{Connection, logical_id},
            fetch_handle::{FetchHandle, FetchRemainder},
            namespace_tuple,
            session_context::{CarriedSubscription, SessionContext},
        },
        runtime::dispatch::incoming_object::IncomingObject,
        version::{Notification, OutgoingRequest, ReceivedResponse},
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
        let mut carried = self.session.lock_carried().await;
        send_subscribe_namespace(&self.session.connection(), namespace.clone()).await?;
        if let Some(carried) = carried.as_mut() {
            carried.namespaces.push(namespace);
        }
        Ok(())
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
        let mut carried = self.session.lock_carried().await;
        let connection = self.session.connection();
        let (message, response) =
            send_subscribe(&connection, &track_namespace, &track_name, &option).await?;
        let logical_alias = match self
            .session
            .register_data_receiver(&connection, message.request_id, message.track_alias)
            .await
        {
            Ok(logical_alias) => logical_alias,
            Err(code) => {
                // The track alias is already bound to another active subscription.
                // draft-14 §9.8: the subscriber MUST close with DUPLICATE_TRACK_ALIAS.
                tracing::error!(
                    track_alias = message.track_alias,
                    "SUBSCRIBE_OK reused an in-use track alias; closing session"
                );
                self.session.close_connection(
                    &connection,
                    code,
                    "SUBSCRIBE_OK reused an in-use track alias",
                );
                // The close ends the subscription; no UNSUBSCRIBE is needed.
                response.accept();
                return Err(RequestFailure::ProtocolViolation(
                    "SUBSCRIBE_OK reused an in-use track alias",
                ));
            }
        };
        tracing::debug!(
            track_alias = message.track_alias,
            "subscriber registered incoming object receiver after SUBSCRIBE_OK"
        );
        let request_id = logical_id(connection.generation, message.request_id);
        if let Some(carried) = carried.as_mut() {
            carried.subscriptions.insert(
                request_id,
                CarriedSubscription {
                    track_namespace: track_namespace.clone(),
                    track_name: track_name.clone(),
                    option: option.clone(),
                },
            );
        }
        let message = SubscribeOk {
            request_id,
            track_alias: logical_alias,
            ..message
        };
        response.accept();
        Ok(Subscription::SubscriberInitiated(
            SubscriberInitiatedSubscription::new(
                track_namespace,
                track_name,
                message,
                option.subscriber_priority,
                option.filter_type,
            ),
        ))
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
        let connection = self.session.connection();
        let vec_namespace = namespace_tuple(&track_namespace);
        let request_id = connection.control.next_request_id();
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
        let response = connection
            .control
            .send_request(OutgoingRequest::TrackStatus(track_status))
            .await?
            .response()
            .await?
            .accept();
        match response {
            ResponseMessage::TrackStatusOk(message) => {
                if request_id != message.request_id {
                    return Err(RequestFailure::ProtocolViolation(
                        "response for another request ID",
                    ));
                }
                tracing::info!(request_id, "TRACK_STATUS_OK received");
                Ok(message)
            }
            ResponseMessage::TrackStatusError(request_id, error_code, reason_phrase) => {
                tracing::info!(
                    request_id,
                    ?error_code,
                    %reason_phrase,
                    "TRACK_STATUS_ERROR received"
                );
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
        let remainder = FetchRemainder {
            track_namespace: track_namespace.clone(),
            track_name: track_name.clone(),
            start: start_location,
            subscriber_priority: option.subscriber_priority,
        };
        let fetch_params = FetchParams::Standalone {
            track_namespace: namespace_tuple(&track_namespace),
            track_name,
            start_location,
            end_location,
        };
        self.send_fetch(
            self.session.connection(),
            fetch_params,
            option,
            Some(remainder),
        )
        .await
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.fetch_cancel",
        skip_all,
        fields(request_id = %fetch.request_id)
    )]
    pub async fn fetch_cancel(&self, fetch: &FetchHandle) -> anyhow::Result<()> {
        let Some((connection, request_id)) = self.on_live_connection(fetch.request_id) else {
            return Ok(());
        };
        self.session.remove_pending_fetch(&connection, request_id);
        connection
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
        let (connection, joining_request_id) = self
            .on_live_connection(subscription.request_id())
            .unwrap_or_else(|| (self.session.connection(), subscription.request_id()));
        let remainder = match subscription.content_exists() {
            ContentExists::True { location } => Some(FetchRemainder {
                track_namespace: subscription.track_namespace().to_string(),
                track_name: subscription.track_name().to_string(),
                start: Location {
                    group_id: location.group_id.saturating_sub(joining_start),
                    object_id: 0,
                },
                subscriber_priority: option.subscriber_priority,
            }),
            ContentExists::False => None,
        };
        let fetch_params = FetchParams::RelativeJoining {
            joining_request_id,
            joining_start,
        };
        self.send_fetch(connection, fetch_params, option, remainder)
            .await
    }

    /// `remainder` is what a session that follows GOAWAY asks the next
    /// connection for when the response stream is cut off; only an
    /// ascending FETCH is sent again.
    async fn send_fetch(
        &self,
        connection: Arc<Connection>,
        fetch_params: FetchParams,
        option: FetchOption,
        remainder: Option<FetchRemainder>,
    ) -> Result<FetchHandle, RequestFailure> {
        let remainder = remainder.filter(|_| {
            self.session.follows_go_away() && option.group_order != GroupOrder::Descending
        });
        let request_id = connection.control.next_request_id();

        let pending_fetch = self.session.register_pending_fetch(&connection, request_id);

        let fetch = Fetch {
            request_id,
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order,
            fetch_params,
            authorization_tokens: vec![],
        };
        let response = connection
            .control
            .send_request(OutgoingRequest::Fetch(fetch))
            .await?
            .response()
            .await?
            .accept();
        match response {
            ResponseMessage::FetchOk(fetch_ok) => {
                pending_fetch.keep();
                let mut handle = FetchHandle::new(FetchOk {
                    request_id: logical_id(connection.generation, fetch_ok.request_id),
                    ..fetch_ok
                });
                handle.remainder = remainder;
                Ok(handle)
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

    /// Sends a SUBSCRIBE_UPDATE for `subscription` on each connection that
    /// carries it (draft-14 §9.10).
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
        let mut carried = self.session.lock_carried().await;
        if let Some(carried) = carried
            .as_mut()
            .and_then(|carried| carried.subscriptions.get_mut(&subscription.request_id()))
        {
            carried.option.forward = option.forward;
            carried.option.subscriber_priority = option.subscriber_priority;
            if let Some(end_group) = option.end_group.checked_sub(1) {
                carried.option.filter_type = FilterType::AbsoluteRange {
                    location: option.start_location,
                    end_group,
                };
            }
        }
        for (connection, subscription_request_id) in
            self.on_live_connections(subscription.request_id())
        {
            let subscribe_update = SubscribeUpdate {
                request_id: connection.control.next_request_id(),
                subscription_request_id,
                start_location: option.start_location,
                end_group: option.end_group,
                subscriber_priority: option.subscriber_priority,
                forward: option.forward,
                authorization_tokens: vec![],
                delivery_timeout: None,
            };
            connection
                .control
                .notify(Notification::SubscribeUpdate(subscribe_update))
                .await?;
        }
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
        if let Some(carried) = self.session.lock_carried().await.as_mut() {
            carried.subscriptions.remove(&subscription.request_id());
        }
        for (connection, request_id) in self.on_live_connections(subscription.request_id()) {
            self.session
                .cancel_subscription(&connection, request_id)
                .await;
            connection
                .control
                .notify(Notification::Unsubscribe(Unsubscribe { request_id }))
                .await?;
        }
        Ok(())
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.unsubscribe_namespace",
        skip_all,
        fields(namespace = %namespace)
    )]
    pub async fn unsubscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        if let Some(carried) = self.session.lock_carried().await.as_mut() {
            carried.namespaces.retain(|carried| *carried != namespace);
        }
        let vec_namespace = namespace_tuple(&namespace);
        for connection in self.session.live_connections() {
            connection
                .control
                .notify(Notification::UnsubscribeNamespace(
                    UnsubscribeNamespace::new(vec_namespace.clone()),
                ))
                .await?;
        }
        Ok(())
    }

    /// The objects of `subscription`. The wait for the first data stream runs
    /// in the reader's task, so unlike `accept_data_receiver`,
    /// `TrackReader::next_object` may be polled in `select!` from the start.
    /// A track delivered in datagrams yields a `MalformedTrackError`.
    pub fn track_reader(&self, subscription: &Subscription) -> TrackReader {
        TrackReader::of(
            Subscriber {
                session: self.session.clone(),
            },
            subscription.clone(),
        )
    }

    /// Subscribes with the Largest Object filter and reads the group that
    /// object is in from its first object, through a Relative Joining FETCH
    /// with offset 0 (draft-14 §9.16.2), so the FETCH and the subscription
    /// meet without a gap or a repeat. The reader yields the fetched objects
    /// before the live ones. A track without content, or a FETCH that fails,
    /// yields the live objects only.
    pub async fn subscribe_current_group(
        &mut self,
        track_namespace: String,
        track_name: String,
    ) -> Result<(Subscription, TrackReader), RequestFailure> {
        let option = SubscribeOption {
            filter_type: FilterType::LargestObject,
            ..SubscribeOption::default()
        };
        let subscription = self.subscribe(track_namespace, track_name, option).await?;
        let reader = TrackReader::joining_current_group(
            Subscriber {
                session: self.session.clone(),
            },
            subscription.clone(),
        );
        Ok((subscription, reader))
    }

    /// Each live connection that carries the request `logical_id`, with its
    /// wire id there.
    fn on_live_connections(&self, logical_id: u64) -> Vec<(Arc<Connection>, u64)> {
        self.session
            .live_connections()
            .into_iter()
            .filter_map(|connection| {
                let wire_id = connection.wire_request_id(logical_id)?;
                Some((connection, wire_id))
            })
            .collect()
    }

    fn on_live_connection(&self, logical_id: u64) -> Option<(Arc<Connection>, u64)> {
        self.on_live_connections(logical_id).into_iter().next()
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
        let mut receiver = fetch_stream_rx
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Failed to receive fetch stream"))?;
        if let Some(remainder) = fetch_handle.remainder.clone() {
            receiver.carry_with(
                self.session.clone(),
                remainder,
                fetch_handle.end_location,
                fetch_handle.request_id,
            );
        }
        Ok(receiver)
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

pub(crate) async fn send_subscribe_namespace(
    connection: &Connection,
    namespace: String,
) -> Result<(), RequestFailure> {
    let vec_namespace = namespace_tuple(&namespace);
    let request_id = connection.control.next_request_id();
    let pending = connection
        .control
        .send_request(OutgoingRequest::SubscribeNamespace(
            SubscribeNamespace::new(request_id, vec_namespace, vec![]),
        ))
        .await?;
    tracing::debug!(request_id, track_namespace_prefix = %namespace, "SUBSCRIBE_NAMESPACE sent");
    let response = pending.response().await?.accept();
    match response {
        ResponseMessage::SubscribeNameSpaceOk(response_request_id) => {
            if request_id != response_request_id {
                Err(RequestFailure::ProtocolViolation(
                    "response for another request ID",
                ))
            } else {
                tracing::info!(
                    request_id,
                    track_namespace_prefix = %namespace,
                    "SUBSCRIBE_NAMESPACE_OK received"
                );
                Ok(())
            }
        }
        ResponseMessage::SubscribeNameSpaceError(request_id, error_code, reason_phrase) => {
            tracing::info!(
                request_id,
                track_namespace_prefix = %namespace,
                ?error_code,
                %reason_phrase,
                "SUBSCRIBE_NAMESPACE_ERROR received"
            );
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

/// The SUBSCRIBE_OK, with `connection`'s wire ids, and its
/// `ReceivedResponse`: dropping that before `accept` sends UNSUBSCRIBE.
pub(crate) async fn send_subscribe(
    connection: &Connection,
    track_namespace: &str,
    track_name: &str,
    option: &SubscribeOption,
) -> Result<(SubscribeOk, ReceivedResponse), RequestFailure> {
    let request_id = connection.control.next_request_id();
    let subscribe = Subscribe {
        request_id,
        track_namespace: namespace_tuple(track_namespace),
        track_name: track_name.to_string(),
        subscriber_priority: option.subscriber_priority,
        group_order: option.group_order,
        forward: option.forward,
        filter_type: option.filter_type,
        authorization_tokens: vec![],
        delivery_timeout: None,
    };
    let response = connection
        .control
        .send_request(OutgoingRequest::Subscribe(subscribe))
        .await?
        .response()
        .await?;
    if let ResponseMessage::SubscribeOk(message) = response.message()
        && message.request_id == request_id
    {
        tracing::info!(
            request_id,
            track_namespace,
            track_name,
            track_alias = message.track_alias,
            "SUBSCRIBE_OK received"
        );
        return Ok((message.clone(), response));
    }
    match response.accept() {
        ResponseMessage::SubscribeOk(_) => Err(RequestFailure::ProtocolViolation(
            "response for another request ID",
        )),
        ResponseMessage::SubscribeError(request_id, error_code, reason_phrase) => {
            tracing::info!(
                request_id,
                track_namespace,
                track_name,
                ?error_code,
                %reason_phrase,
                "SUBSCRIBE_ERROR received"
            );
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

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures::FutureExt;

    use tokio::task::JoinHandle;

    use crate::{
        RequestErrorCode, RequestFailure, Session, SessionEvent, Subscription,
        TerminationErrorCode, TrackReader,
        fetch::{
            Fetch, FetchCancelHandler, FetchDataReceiver, FetchDataSender, FetchHandle,
            FetchHandler, FetchOption, FetchParams,
        },
        modules::{
            moqt::domains::connection::IncomingObjectNotification,
            test_support::{
                DISCONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, first_subgroup_stream, location,
                notify_datagram, object_with_extensions, read_fetch_objects, read_subgroup_objects,
                send_subgroup_object, send_without_waiting, spawn_connected_dual_sessions,
                subgroup_receiver, subscription, wait_for_control_delivery, within,
            },
            transport::transport_send_stream::TransportSendError,
        },
        object::{
            ContentExists, ExtensionHeaders, ForwardingPreference, Location, Object, ObjectStatus,
        },
        subscribe::{SubscribeHandler, SubscribeOption, SubscribeUpdateOption},
    };

    const START: Location = Location {
        group_id: 0,
        object_id: 0,
    };
    const END: Location = Location {
        group_id: 1,
        object_id: 0,
    };
    const TRACK: &str = "track";
    /// Exceeds quinn's default limit of 100 open streams per type, so a FETCH
    /// stream that is never finished or stopped stalls a later round. Client
    /// request ids step by 2, so 150 FETCHes stay below the MAX_REQUEST_ID of
    /// 1000 both sessions are set up with.
    const REPEATED_FETCHES: usize = 150;

    fn fetch_object(location: Location, payload: &'static [u8]) -> Object {
        Object {
            location,
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: 128,
            status: ObjectStatus::Normal,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from_static(payload),
            ends_group: false,
        }
    }

    async fn start_fetch(
        client: &Session,
        server: &Session,
        track_name: &str,
    ) -> (
        JoinHandle<Result<FetchHandle, RequestFailure>>,
        FetchHandler,
    ) {
        let mut subscriber = client.subscriber();
        let track_name = track_name.to_string();
        let request = tokio::spawn(async move {
            subscriber
                .fetch(
                    "ns".to_string(),
                    track_name,
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
            .connection()
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

    async fn serve_fetch(
        server: &Session,
        handler: &FetchHandler,
        end: Location,
        objects: &[Object],
    ) {
        handler.ok(false, end).await.unwrap();
        let sender = server
            .publisher()
            .create_fetch_stream(handler.request_id)
            .await
            .unwrap();
        for object in objects {
            sender.send_object(object).await.unwrap();
        }
        sender.close().await.unwrap();
    }

    async fn receive_fetch(
        client: &Session,
        request: JoinHandle<Result<FetchHandle, RequestFailure>>,
    ) -> (FetchHandle, Vec<Object>) {
        let handle = request.await.unwrap().unwrap();
        let mut receiver = client
            .subscriber()
            .accept_fetch_receiver(&handle)
            .await
            .unwrap();
        let objects = read_fetch_objects(&mut receiver).await;
        (handle, objects)
    }

    async fn complete_fetch(
        client: &Session,
        server: &Session,
        objects: &[Object],
    ) -> (FetchHandle, Vec<Object>) {
        let (request, handler) = start_fetch(client, server, TRACK).await;
        serve_fetch(server, &handler, END, objects).await;
        receive_fetch(client, request).await
    }

    struct OpenFetch {
        handle: FetchHandle,
        handler: FetchHandler,
        sender: FetchDataSender,
        receiver: FetchDataReceiver,
    }

    /// A FETCH answered with FETCH_OK whose stream carries `first` and stays
    /// open.
    async fn open_fetch(client: &Session, server: &Session, first: &Object) -> OpenFetch {
        let (request, handler) = start_fetch(client, server, TRACK).await;
        handler.ok(false, END).await.unwrap();
        let sender = server
            .publisher()
            .create_fetch_stream(handler.request_id)
            .await
            .unwrap();
        sender.send_object(first).await.unwrap();
        let handle = request.await.unwrap().unwrap();
        let receiver = client
            .subscriber()
            .accept_fetch_receiver(&handle)
            .await
            .unwrap();
        OpenFetch {
            handle,
            handler,
            sender,
            receiver,
        }
    }

    async fn receive_fetch_cancel(server: &Session) -> FetchCancelHandler {
        let SessionEvent::FetchCancel(cancel) = server.receive_event().await.unwrap() else {
            panic!("expected FETCH_CANCEL from the client");
        };
        cancel
    }

    async fn send_until_stopped(sender: &FetchDataSender) -> anyhow::Error {
        loop {
            if let Err(error) = sender.send_object(&fetch_object(START, b"unwanted")).await {
                return error;
            }
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn accepted_fetch_releases_its_stream_sender_and_keeps_delivering_objects() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-fetch-sender").await;
        let (request, handler) = start_fetch(&client, &server, TRACK).await;
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
        fetch_sender
            .send_object(&fetch_object(START, b"object"))
            .await
            .unwrap();
        let object = fetch_receiver.receive().await.unwrap();

        // Assert
        assert!(
            !client
                .subscriber()
                .session
                .connection()
                .fetch_notification_map
                .lock()
                .unwrap()
                .contains_key(&fetch_handle.request_id)
        );
        assert!(
            matches!(&object, Fetch::Object(received) if *received == fetch_object(START, b"object")),
            "{object:?}"
        );
    }

    #[tokio::test]
    async fn fetch_receiver_fails_when_the_peer_disconnects_before_the_stream() {
        // Arrange
        let (client, server) =
            spawn_connected_dual_sessions("subscriber-fetch-disconnect-before-stream").await;
        let (request, handler) = start_fetch(&client, &server, TRACK).await;
        handler.ok(false, END).await.unwrap();
        let fetch_handle = request.await.unwrap().unwrap();
        let mut subscriber = client.subscriber();

        // Act
        server.close_with_error(TerminationErrorCode::NoError, "closing");
        let accepted = tokio::time::timeout(
            DISCONNECT_TIMEOUT,
            subscriber.accept_fetch_receiver(&fetch_handle),
        )
        .await;

        // Assert
        let accepted = accepted.expect("the stream wait ends once the peer disconnects");
        assert!(accepted.is_err(), "{accepted:?}");
    }

    #[tokio::test]
    async fn fetch_ok_hands_its_max_cache_duration_to_the_fetch_handle() {
        // Arrange
        let (client, server) =
            spawn_connected_dual_sessions("subscriber-fetch-max-cache-duration").await;
        let (request, mut handler) = start_fetch(&client, &server, TRACK).await;
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
        let (request, handler) = start_fetch(&client, &server, TRACK).await;

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
        let (request, handler) = start_fetch(&client, &server, TRACK).await;

        // Act
        request.abort();
        let _ = request.await;

        // Assert
        assert!(!has_pending_fetch(&client, handler.request_id));
    }

    #[tokio::test]
    async fn repeated_fetches_on_one_session_each_end_and_release_their_state() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-fetch-repeated").await;
        let with_objects = [
            fetch_object(location(0, 0), b"first"),
            fetch_object(location(0, 1), b"second"),
        ];
        let rounds: Vec<&[Object]> = (0..REPEATED_FETCHES)
            .map(|round| {
                if round % 2 == 0 {
                    &with_objects[..]
                } else {
                    &[]
                }
            })
            .collect();

        // Act
        let mut fetched = Vec::new();
        for (round, objects) in rounds.iter().enumerate() {
            let completed =
                tokio::time::timeout(HANDSHAKE_TIMEOUT, complete_fetch(&client, &server, objects))
                    .await
                    .unwrap_or_else(|_| panic!("FETCH round {round} did not complete"));
            fetched.push(completed);
        }

        // Assert
        for (round, ((_, received), sent)) in fetched.iter().zip(&rounds).enumerate() {
            assert_eq!(received.as_slice(), *sent, "round {round}");
        }
        for (handle, _) in &fetched {
            assert!(!has_pending_fetch(&client, handle.request_id));
        }
    }

    #[tokio::test]
    async fn fetch_ok_after_the_caller_stopped_waiting_cancels_the_fetch_and_later_fetches_succeed()
    {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-fetch-late-ok").await;
        let (request, handler) = start_fetch(&client, &server, TRACK).await;
        request.abort();
        let _ = request.await;
        let later_objects = [fetch_object(START, b"later")];

        // Act
        handler.ok(false, END).await.unwrap();
        let sender = server
            .publisher()
            .create_fetch_stream(handler.request_id)
            .await
            .unwrap();
        let cancel = within(receive_fetch_cancel(&server)).await;
        let stopped = within(send_until_stopped(&sender)).await;
        let (_, later) = within(complete_fetch(&client, &server, &later_objects)).await;

        // Assert
        assert_eq!(cancel.request_id(), handler.request_id);
        assert!(
            TransportSendError::is_stopped_by_peer(&stopped),
            "{stopped:#}"
        );
        assert!(!has_pending_fetch(&client, handler.request_id));
        assert_eq!(later, later_objects);
    }

    #[tokio::test]
    async fn fetches_answered_in_reverse_order_each_get_their_own_response_and_objects() {
        // Arrange
        let (client, server) =
            spawn_connected_dual_sessions("subscriber-fetch-reverse-order").await;
        let tracks = ["track-0", "track-1", "track-2"];
        let mut requests = Vec::new();
        for track in tracks {
            requests.push(start_fetch(&client, &server, track).await);
        }
        let end_of = |index: usize| location(index as u64 + 1, 0);
        let object_of =
            |index: usize| fetch_object(location(index as u64, 0), tracks[index].as_bytes());

        // Act
        for (index, (_, handler)) in requests.iter().enumerate().rev() {
            within(serve_fetch(
                &server,
                handler,
                end_of(index),
                &[object_of(index)],
            ))
            .await;
        }
        let mut fetched = Vec::new();
        for (request, handler) in requests {
            fetched.push((
                handler.request_id,
                within(receive_fetch(&client, request)).await,
            ));
        }

        // Assert
        for (index, (request_id, (handle, objects))) in fetched.into_iter().enumerate() {
            assert_eq!(handle.request_id, request_id, "{}", tracks[index]);
            assert_eq!(handle.end_location, end_of(index), "{}", tracks[index]);
            assert_eq!(objects, [object_of(index)], "{}", tracks[index]);
        }
    }

    #[tokio::test]
    async fn cancelling_one_fetch_leaves_the_other_fetches_on_the_session_running() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-fetch-cancel-one").await;
        let firsts = [b"a0", b"b0", b"c0"].map(|payload| fetch_object(location(0, 0), payload));
        let seconds = [b"a1", b"c1"].map(|payload| fetch_object(location(0, 1), payload));
        let mut kept_a = open_fetch(&client, &server, &firsts[0]).await;
        let cancelled = open_fetch(&client, &server, &firsts[1]).await;
        let mut kept_c = open_fetch(&client, &server, &firsts[2]).await;

        // Act
        client
            .subscriber()
            .fetch_cancel(&cancelled.handle)
            .await
            .unwrap();
        drop(cancelled.receiver);
        let cancel = within(receive_fetch_cancel(&server)).await;
        let stopped = within(send_until_stopped(&cancelled.sender)).await;
        for (kept, second) in [(&kept_a, &seconds[0]), (&kept_c, &seconds[1])] {
            kept.sender.send_object(second).await.unwrap();
            kept.sender.close().await.unwrap();
        }
        let received_a = within(read_fetch_objects(&mut kept_a.receiver)).await;
        let received_c = within(read_fetch_objects(&mut kept_c.receiver)).await;

        // Assert
        assert_eq!(cancel.request_id(), cancelled.handler.request_id);
        assert!(
            TransportSendError::is_stopped_by_peer(&stopped),
            "{stopped:#}"
        );
        assert_eq!(received_a, [firsts[0].clone(), seconds[0].clone()]);
        assert_eq!(received_c, [firsts[2].clone(), seconds[1].clone()]);
    }

    #[tokio::test]
    async fn fetch_fails_as_soon_as_the_peer_disconnects_without_answering() {
        // Arrange
        let (client, server) =
            spawn_connected_dual_sessions("subscriber-fetch-disconnect-response").await;
        let (request, _handler) = start_fetch(&client, &server, TRACK).await;

        // Act
        server.close_with_error(TerminationErrorCode::NoError, "closing");
        let fetched = tokio::time::timeout(DISCONNECT_TIMEOUT, request).await;

        // Assert
        let fetched = fetched
            .expect("the FETCH wait ends once the peer disconnects")
            .unwrap();
        assert!(
            matches!(fetched, Err(RequestFailure::SessionClosed)),
            "{fetched:?}"
        );
    }

    #[tokio::test]
    async fn fetch_stream_fails_instead_of_ending_when_the_peer_disconnects_mid_stream() {
        // Arrange
        let (client, server) =
            spawn_connected_dual_sessions("subscriber-fetch-disconnect-mid-stream").await;
        let mut open = open_fetch(&client, &server, &fetch_object(START, b"first")).await;
        let Fetch::Object(_) = within(open.receiver.receive()).await.unwrap() else {
            panic!("expected the first object of the FETCH");
        };

        // Act
        server.close_with_error(TerminationErrorCode::NoError, "closing");
        let received = tokio::time::timeout(DISCONNECT_TIMEOUT, open.receiver.receive()).await;

        // Assert
        let received = received.expect("the receive wait ends once the peer disconnects");
        assert!(received.is_err(), "{received:?}");
    }

    async fn start_subscribe(
        client: &Session,
        server: &Session,
        track_name: &str,
    ) -> (
        JoinHandle<Result<Subscription, RequestFailure>>,
        SubscribeHandler,
    ) {
        let mut subscriber = client.subscriber();
        let track_name = track_name.to_string();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe("ns".to_string(), track_name, SubscribeOption::default())
                .await
        });
        let SessionEvent::Subscribe(handler) = within(server.receive_event()).await.unwrap() else {
            panic!("expected SUBSCRIBE from the client");
        };
        (request, handler)
    }

    async fn receive_subscribe_objects(
        client: &Session,
        subscription: &Subscription,
    ) -> (u64, Vec<Object>) {
        let mut receiver = first_subgroup_stream(client, subscription).await;
        let objects = read_subgroup_objects(&mut receiver).await;
        (receiver.track_alias, objects)
    }

    #[tokio::test]
    async fn subscribe_ok_after_cancellation_unsubscribes_and_later_subscribe_delivers() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-subscribe-late-ok").await;
        let (request, handler) = start_subscribe(&client, &server, "cancelled").await;
        let mut object = object_with_extensions();
        object.payload = Bytes::from_static(b"later subscription");

        // Act
        request.abort();
        let cancelled = request.await.unwrap_err();
        let unsubscribe = within(async {
            handler.ok(0, ContentExists::False).await.unwrap();
            let SessionEvent::Unsubscribe(unsubscribe) = server.receive_event().await.unwrap()
            else {
                panic!("expected UNSUBSCRIBE after a late SUBSCRIBE_OK");
            };
            unsubscribe
        })
        .await;
        let (later_request, later_handler) = start_subscribe(&client, &server, "later").await;
        let (later, received_alias, objects) = within(async {
            let alias = later_handler.ok(0, ContentExists::False).await.unwrap();
            let later = later_request.await.unwrap().unwrap();
            send_subgroup_object(&server, &later_handler.into_subscription(alias), &object).await;
            let (received_alias, objects) = receive_subscribe_objects(&client, &later).await;
            (later, received_alias, objects)
        })
        .await;

        // Assert
        assert!(cancelled.is_cancelled());
        assert_eq!(unsubscribe.subscribe_id(), handler.request_id());
        assert_ne!(later.request_id(), handler.request_id());
        assert_eq!(later.track_name(), "later");
        assert_eq!(received_alias, later.track_alias());
        assert_eq!(objects, [object]);
    }

    #[tokio::test]
    async fn unsubscribing_one_track_leaves_the_other_delivering_on_the_same_session() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-unsubscribe-one").await;
        let (cancelled_request, cancelled_handler) =
            start_subscribe(&client, &server, "cancelled").await;
        let cancelled_alias = within(cancelled_handler.ok(0, ContentExists::False))
            .await
            .unwrap();
        let cancelled = within(cancelled_request).await.unwrap().unwrap();
        let (kept_request, kept_handler) = start_subscribe(&client, &server, "kept").await;
        let kept_alias = within(kept_handler.ok(0, ContentExists::False))
            .await
            .unwrap();
        let kept = within(kept_request).await.unwrap().unwrap();
        let mut before = object_with_extensions();
        before.payload = Bytes::from_static(b"before unsubscribe");
        within(send_subgroup_object(
            &server,
            &cancelled_handler.into_subscription(cancelled_alias),
            &before,
        ))
        .await;
        let (_, cancelled_before) = within(receive_subscribe_objects(&client, &cancelled)).await;
        within(send_subgroup_object(
            &server,
            &kept_handler.into_subscription(kept_alias),
            &before,
        ))
        .await;
        let mut kept_receiver = within(subgroup_receiver(&client, &kept)).await;
        let mut first = within(kept_receiver.next()).await.unwrap();
        let kept_before = within(read_subgroup_objects(&mut first)).await;
        let mut after = object_with_extensions();
        after.location.group_id += 1;
        after.payload = Bytes::from_static(b"after unsubscribe");

        // Act
        let (unsubscribe, received_alias, objects) = within(async {
            client.subscriber().unsubscribe(&cancelled).await.unwrap();
            let SessionEvent::Unsubscribe(unsubscribe) = server.receive_event().await.unwrap()
            else {
                panic!("expected UNSUBSCRIBE from the client");
            };
            send_subgroup_object(&server, &kept_handler.into_subscription(kept_alias), &after)
                .await;
            let mut next = kept_receiver.next().await.unwrap();
            let received_alias = next.track_alias;
            let objects = read_subgroup_objects(&mut next).await;
            (unsubscribe, received_alias, objects)
        })
        .await;

        // Assert
        assert_eq!(cancelled_before, [before.clone()]);
        assert_eq!(kept_before, [before]);
        assert_eq!(unsubscribe.subscribe_id(), cancelled_handler.request_id());
        assert_ne!(unsubscribe.subscribe_id(), kept.request_id());
        assert_eq!(received_alias, kept_alias);
        assert_eq!(objects, [after]);
    }

    #[tokio::test]
    async fn subscribes_answered_in_reverse_order_receive_their_own_metadata_alias_and_objects() {
        // Arrange
        let (client, server) =
            spawn_connected_dual_sessions("subscriber-subscribe-reverse-order").await;
        let tracks = ["track-0", "track-1", "track-2"];
        let mut requests = Vec::new();
        for track in tracks {
            requests.push(start_subscribe(&client, &server, track).await);
        }
        let objects = tracks.map(|track| {
            let mut object = object_with_extensions();
            object.payload = Bytes::copy_from_slice(track.as_bytes());
            object
        });
        let content_of = |index: usize| ContentExists::True {
            location: location(index as u64 + 1, 0),
        };
        let mut aliases = vec![0; tracks.len()];

        // Act
        let received = within(async {
            for (index, (_, handler)) in requests.iter_mut().enumerate().rev() {
                handler.max_cache_duration = Some(1000 + index as u64);
                aliases[index] = handler
                    .ok(10 + index as u64, content_of(index))
                    .await
                    .unwrap();
            }
            let mut received = Vec::new();
            for (index, (request, handler)) in requests.into_iter().enumerate() {
                let subscription = request.await.unwrap().unwrap();
                send_subgroup_object(
                    &server,
                    &handler.into_subscription(aliases[index]),
                    &objects[index],
                )
                .await;
                let (received_alias, objects) =
                    receive_subscribe_objects(&client, &subscription).await;
                received.push((handler.request_id(), subscription, received_alias, objects));
            }
            received
        })
        .await;

        // Assert
        for (index, (request_id, subscription, received_alias, received_objects)) in
            received.into_iter().enumerate()
        {
            assert_eq!(subscription.request_id(), request_id, "{}", tracks[index]);
            assert_eq!(subscription.track_namespace(), "ns");
            assert_eq!(subscription.track_name(), tracks[index]);
            assert_eq!(subscription.track_alias(), aliases[index]);
            assert_eq!(received_alias, aliases[index]);
            assert_eq!(subscription.expires(), Some(10 + index as u64));
            assert_eq!(subscription.content_exists(), content_of(index));
            assert_eq!(subscription.max_cache_duration(), Some(1000 + index as u64));
            assert_eq!(received_objects, [objects[index].clone()]);
        }
        assert_ne!(aliases[0], aliases[1]);
        assert_ne!(aliases[1], aliases[2]);
        assert_ne!(aliases[0], aliases[2]);
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
    async fn fetch_dropped_after_its_fetch_ok_arrived_cancels_the_fetch() {
        // Arrange
        let (client, server) =
            spawn_connected_dual_sessions("subscriber-fetch-drop-after-ok").await;
        let mut subscriber = client.subscriber();
        let mut request = Box::pin(subscriber.fetch(
            "ns".to_string(),
            TRACK.to_string(),
            START,
            END,
            FetchOption::default(),
        ));
        let SessionEvent::Fetch(handler) = send_without_waiting(&mut request, &server).await else {
            panic!("expected FETCH from the client");
        };
        handler.ok(false, END).await.unwrap();
        wait_for_control_delivery(&server, &client).await;

        // Act
        drop(request);
        let cancel = within(receive_fetch_cancel(&server)).await;

        // Assert
        assert_eq!(cancel.request_id(), handler.request_id);
    }

    #[tokio::test]
    async fn subscribe_dropped_after_its_subscribe_ok_arrived_unsubscribes() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-drop-after-ok").await;
        let mut subscriber = client.subscriber();
        let mut request = Box::pin(subscriber.subscribe(
            "ns".to_string(),
            TRACK.to_string(),
            SubscribeOption::default(),
        ));
        let SessionEvent::Subscribe(handler) = send_without_waiting(&mut request, &server).await
        else {
            panic!("expected SUBSCRIBE from the client");
        };
        handler.ok(0, ContentExists::False).await.unwrap();
        wait_for_control_delivery(&server, &client).await;

        // Act
        drop(request);
        let event = within(server.receive_event()).await.unwrap();

        // Assert
        let SessionEvent::Unsubscribe(unsubscribe) = event else {
            panic!("expected UNSUBSCRIBE, got {event:?}");
        };
        assert_eq!(unsubscribe.subscribe_id(), handler.request_id());
    }

    #[tokio::test]
    async fn subscribe_dropped_while_registering_its_receiver_unsubscribes() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-drop-registering").await;
        let session = client.subscriber().session;
        let mut subscriber = client.subscriber();
        let mut request = Box::pin(subscriber.subscribe(
            "ns".to_string(),
            TRACK.to_string(),
            SubscribeOption::default(),
        ));
        let SessionEvent::Subscribe(handler) = send_without_waiting(&mut request, &server).await
        else {
            panic!("expected SUBSCRIBE from the client");
        };
        let track_alias = handler.ok(0, ContentExists::False).await.unwrap();
        wait_for_control_delivery(&server, &client).await;
        let receiver_map = session.receiver_map.lock().await;
        assert!(request.as_mut().now_or_never().is_none());

        // Act
        drop(request);
        drop(receiver_map);
        let event = within(server.receive_event()).await.unwrap();
        let notification = notify_datagram(&session, track_alias).await;

        // Assert: the receiver installed before the drop no longer takes objects
        let SessionEvent::Unsubscribe(unsubscribe) = event else {
            panic!("expected UNSUBSCRIBE, got {event:?}");
        };
        assert_eq!(unsubscribe.subscribe_id(), handler.request_id());
        assert!(
            matches!(notification, IncomingObjectNotification::Discarded),
            "{notification:?}"
        );
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

    async fn start_subscribe_current_group(
        client: &Session,
        server: &Session,
        content_exists: ContentExists,
    ) -> (TrackReader, Subscription) {
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe_current_group("ns".to_string(), TRACK.to_string())
                .await
        });
        let SessionEvent::Subscribe(handler) = within(server.receive_event()).await.unwrap() else {
            panic!("expected SUBSCRIBE from the client");
        };
        let alias = handler.ok(0, content_exists).await.unwrap();
        let (_, reader) = request.await.unwrap().unwrap();
        (reader, handler.into_subscription(alias))
    }

    async fn read_payloads(reader: &mut TrackReader, count: usize) -> Vec<Bytes> {
        within(async {
            let mut payloads = Vec::new();
            while payloads.len() < count {
                payloads.push(reader.next_object().await.unwrap().unwrap().payload);
            }
            payloads
        })
        .await
    }

    #[tokio::test]
    async fn subscribe_current_group_yields_the_fetched_group_before_a_live_object_that_arrived_first()
     {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-current-group").await;
        let (mut reader, served) = start_subscribe_current_group(
            &client,
            &server,
            ContentExists::True {
                location: location(3, 1),
            },
        )
        .await;
        send_subgroup_object(&server, &served, &fetch_object(location(3, 2), b"live")).await;

        // Act
        let SessionEvent::Fetch(fetch) = within(server.receive_event()).await.unwrap() else {
            panic!("expected the joining FETCH");
        };
        serve_fetch(
            &server,
            &fetch,
            location(3, 2),
            &[
                fetch_object(location(3, 0), b"fetched-0"),
                fetch_object(location(3, 1), b"fetched-1"),
            ],
        )
        .await;
        let payloads = read_payloads(&mut reader, 3).await;

        // Assert
        assert!(matches!(
            fetch.fetch.fetch_params,
            FetchParams::RelativeJoining {
                joining_start: 0,
                ..
            }
        ));
        assert_eq!(payloads, ["fetched-0", "fetched-1", "live"]);
    }

    #[tokio::test]
    async fn subscribe_current_group_yields_live_objects_when_the_joining_fetch_is_refused() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-current-refused").await;
        let (mut reader, served) = start_subscribe_current_group(
            &client,
            &server,
            ContentExists::True {
                location: location(3, 1),
            },
        )
        .await;
        let SessionEvent::Fetch(fetch) = within(server.receive_event()).await.unwrap() else {
            panic!("expected the joining FETCH");
        };

        // Act
        fetch
            .error(RequestErrorCode::NotSupported, String::new())
            .await
            .unwrap();
        send_subgroup_object(&server, &served, &fetch_object(location(3, 2), b"live")).await;
        let payloads = read_payloads(&mut reader, 1).await;

        // Assert
        assert_eq!(payloads, ["live"]);
    }

    #[tokio::test]
    async fn subscribe_current_group_sends_no_fetch_for_a_track_without_content() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("subscriber-current-empty").await;
        let (mut reader, served) =
            start_subscribe_current_group(&client, &server, ContentExists::False).await;

        // Act
        send_subgroup_object(&server, &served, &fetch_object(location(0, 0), b"live")).await;
        let payloads = read_payloads(&mut reader, 1).await;
        let event = server.receive_event().now_or_never();

        // Assert
        assert_eq!(payloads, ["live"]);
        assert!(event.is_none(), "{event:?}");
    }
}
