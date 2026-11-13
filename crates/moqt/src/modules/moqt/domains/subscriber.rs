use crate::modules::telemetry::current_trace_context;
use std::sync::Arc;

use crate::{
    RequestErrorCode, RequestFailure, SubscriberInitiatedSubscription, Subscription,
    TrackProperties, TrackReader,
    fetch::FetchOption,
    modules::moqt::{
        control_plane::{
            control_messages::messages::{
                fetch::Fetch, fetch::FetchParams, fetch_cancel::FetchCancel, fetch_ok::FetchOk,
                parameters::authorization_token::AuthorizationToken,
                parameters::content_exists::ContentExists, subscribe::Subscribe,
                subscribe_namespace::SubscribeNamespace, subscribe_ok::SubscribeOk,
                subscribe_update::SubscribeUpdate, unsubscribe::Unsubscribe,
            },
            enums::{RequestId, ResponseMessage},
            namespace_interest::NamespaceInterest,
        },
        data_plane::{data_receiver::DataReceiver, stream::fetch_data_receiver::FetchDataReceiver},
        domains::{
            connection::{Connection, generation_of, logical_id},
            elapsed_ms,
            fetch_handle::{FetchHandle, FetchRemainder},
            namespace_tuple,
            request_failure::record_outcome,
            session_context::{CarriedSubscription, PendingFetch, SessionContext},
        },
        version::{Notification, OutgoingRequest, PendingResponse, ReceivedResponse},
    },
    object::{GroupOrder, Location},
    subscribe::{FilterType, SubscribeOption, SubscribeUpdateOption},
    wire::TrackStatusOk,
};

pub struct Subscriber {
    pub(crate) session: Arc<SessionContext>,
}

impl Subscriber {
    /// Asks the peer for the namespaces and/or tracks under `namespace`, as
    /// `interest` says. On draft-18 `NamespacesAndTracks` takes both
    /// SUBSCRIBE_NAMESPACE and SUBSCRIBE_TRACKS (§6.1): when either fails,
    /// the other is withdrawn and the failure returned.
    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.subscribe_namespace",
        skip_all,
        fields(
            track_namespace_prefix = %namespace,
            interest = ?interest,
            request_id = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    pub async fn subscribe_namespace(
        &self,
        namespace: String,
        interest: NamespaceInterest,
    ) -> Result<(), RequestFailure> {
        let result = self.subscribe_namespace_answered(namespace, interest).await;
        record_outcome(&result);
        result
    }

    /// Sends the requests `interest` takes on the current connection's draft
    /// and waits for all of them; when one fails, those accepted are
    /// withdrawn.
    async fn subscribe_namespace_answered(
        &self,
        namespace: String,
        interest: NamespaceInterest,
    ) -> Result<(), RequestFailure> {
        let requests = self
            .session
            .connection()
            .control
            .namespace_requests(interest);
        let results = futures::future::join_all(
            requests
                .iter()
                .map(|&request| self.subscribe_namespace_request(namespace.clone(), request)),
        )
        .await;
        let (accepted, failed): (Vec<_>, Vec<_>) = requests
            .into_iter()
            .zip(results)
            .partition(|(_, result)| result.is_ok());
        let Some((_, Err(failure))) = failed.into_iter().next() else {
            return Ok(());
        };
        for (request, _) in accepted {
            if let Err(error) = self.withdraw_namespace_request(&namespace, request).await {
                tracing::debug!(%error, ?request, "could not withdraw a namespace subscription");
            }
        }
        Err(failure)
    }

    async fn subscribe_namespace_request(
        &self,
        namespace: String,
        interest: NamespaceInterest,
    ) -> Result<(), RequestFailure> {
        let (connection, request_id, pending) = loop {
            let carried = self.session.read_carried().await;
            let connection = self.session.connection();
            match begin_subscribe_namespace(&connection, &namespace, interest).await {
                Ok((request_id, pending)) => {
                    // Recorded under the lock it was sent with, before its answer: a
                    // migration sends it again, and an UNSUBSCRIBE_NAMESPACE sent
                    // meanwhile removes it.
                    if let Some(carried) = &carried {
                        let request_id = logical_id(connection.generation, request_id);
                        carried.update(|carried| {
                            carried
                                .namespaces
                                .insert(request_id, (namespace.clone(), interest))
                        });
                    }
                    break (connection, request_id, pending);
                }
                Err(error) => {
                    self.session
                        .resend_after(carried, &connection, error)
                        .await?
                }
            }
        };
        tracing::Span::current().record("request_id", request_id);
        match subscribe_namespace_ok(request_id, &namespace, pending).await {
            Ok(_) => Ok(()),
            // A migration sends it again if it is still carried, and the
            // migration fails unless the new connection accepts it.
            Err(error) if self.session.moved_after_loss(&connection, &error).await => Ok(()),
            Err(error) => {
                if let Some(carried) = self.session.read_carried().await {
                    let request_id = logical_id(connection.generation, request_id);
                    carried.update(|carried| carried.namespaces.remove(&request_id));
                }
                Err(error)
            }
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "moqt.subscriber.subscribe",
        skip_all,
        fields(
            track_namespace = %track_namespace,
            track_name = %track_name,
            subscriber_priority = option.subscriber_priority,
            forward = option.forward,
            request_id = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    pub async fn subscribe(
        &mut self,
        track_namespace: String,
        track_name: String,
        option: SubscribeOption,
    ) -> Result<Subscription, RequestFailure> {
        let result = self
            .subscribe_joining(track_namespace, track_name, option, false)
            .await
            .map(|(subscription, _)| subscription);
        record_outcome(&result);
        result
    }

    /// With `joining`, a Relative Joining FETCH of the current group is written
    /// right behind the SUBSCRIBE on the same connection, before SUBSCRIBE_OK, so
    /// both are answered within one round trip. The peer handles requests in
    /// order, so the FETCH finds the subscription it joins (draft-14 §9.16.2).
    /// The `SentFetch` is `None` without `joining` or when the FETCH could not be
    /// written. Dropping it unanswered withdraws the FETCH.
    async fn subscribe_joining(
        &mut self,
        track_namespace: String,
        track_name: String,
        option: SubscribeOption,
        joining: bool,
    ) -> Result<(Subscription, Option<SentFetch>), RequestFailure> {
        let (carried, connection, message, response, joining_fetch) = loop {
            let carried = self.session.read_carried().await;
            let connection = self.session.connection();
            let sent = async {
                let sent =
                    send_subscribe_request(&connection, &track_namespace, &track_name, &option)
                        .await?;
                tracing::Span::current().record("request_id", sent.request_id);
                let joining_fetch = if joining {
                    self.send_joining_fetch(
                        &connection,
                        sent.request_id,
                        &track_namespace,
                        &track_name,
                    )
                    .await
                    .inspect_err(|error| {
                        tracing::debug!(%error, "joining FETCH could not be sent");
                    })
                    .ok()
                } else {
                    None
                };
                let (message, response) = subscribe_ok(sent, &track_namespace, &track_name).await?;
                Ok((message, response, joining_fetch))
            }
            .await;
            match sent {
                Ok((message, response, joining_fetch)) => {
                    break (carried, connection, message, response, joining_fetch);
                }
                Err(error) => {
                    self.session
                        .resend_after(carried, &connection, error)
                        .await?
                }
            }
        };
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
        if let Some(carried) = &carried {
            carried.update(|carried| {
                carried.subscriptions.insert(
                    request_id,
                    CarriedSubscription {
                        track_namespace: track_namespace.clone(),
                        track_name: track_name.clone(),
                        // A SUBSCRIBE sent again after GOAWAY resumes where
                        // this one got to; it needs no new group.
                        option: SubscribeOption {
                            new_group_request: None,
                            ..option.clone()
                        },
                    },
                )
            });
        }
        let message = SubscribeOk {
            request_id,
            track_alias: logical_alias,
            ..message
        };
        response.accept();
        let subscription = Subscription::SubscriberInitiated(SubscriberInitiatedSubscription::new(
            track_namespace,
            track_name,
            message,
            &option,
        ));
        Ok((subscription, joining_fetch))
    }

    async fn send_joining_fetch(
        &self,
        connection: &Arc<Connection>,
        subscribe_request_id: RequestId,
        track_namespace: &str,
        track_name: &str,
    ) -> Result<SentFetch, RequestFailure> {
        let fetch_params = FetchParams::RelativeJoining {
            joining_request_id: subscribe_request_id,
            joining_start: 0,
        };
        self.send_fetch_request(
            connection.clone(),
            fetch_params,
            &FetchOption::default(),
            (track_namespace.to_string(), track_name.to_string()),
        )
        .await
    }

    #[tracing::instrument(
        level = "debug",
        name = "moqt.subscriber.track_status",
        skip_all,
        fields(
            track_namespace = %track_namespace,
            track_name = %track_name,
            request_id = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    pub async fn track_status(
        &self,
        track_namespace: String,
        track_name: String,
        authorization_tokens: Vec<AuthorizationToken>,
    ) -> Result<TrackStatusOk, RequestFailure> {
        let result = self
            .track_status_answered(track_namespace, track_name, authorization_tokens)
            .await;
        record_outcome(&result);
        result
    }

    async fn track_status_answered(
        &self,
        track_namespace: String,
        track_name: String,
        authorization_tokens: Vec<AuthorizationToken>,
    ) -> Result<TrackStatusOk, RequestFailure> {
        let connection = self.session.connection();
        let vec_namespace = namespace_tuple(&track_namespace);
        let reservation = connection.control.reserve_request_id().await?;
        let request_id = reservation.request_id();
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
            new_group_request: None,
            trace_context: None,
        };
        let pending = connection
            .send_request(reservation, OutgoingRequest::TrackStatus(track_status))
            .await?;
        tracing::Span::current().record("request_id", request_id);
        let response = pending.response().await?.accept();
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
        fields(
            track_namespace = %track_namespace,
            track_name = %track_name,
            request_id = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    pub async fn fetch(
        &mut self,
        track_namespace: String,
        track_name: String,
        start_location: Location,
        end_location: Location,
        option: FetchOption,
    ) -> Result<FetchHandle, RequestFailure> {
        let remainder = FetchRemainder::new(
            track_namespace.clone(),
            track_name.clone(),
            start_location,
            &option,
        );
        let fetch_params = FetchParams::Standalone {
            track_namespace: namespace_tuple(&track_namespace),
            track_name: track_name.clone(),
            start_location,
            end_location,
        };
        let result = self
            .send_fetch(
                self.session.connection(),
                fetch_params,
                option,
                Some(remainder),
                (track_namespace, track_name),
            )
            .await;
        record_outcome(&result);
        result
    }

    #[tracing::instrument(
        level = "debug",
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
        level = "debug",
        name = "moqt.subscriber.fetch_relative_joining",
        skip_all,
        fields(joining_request_id = %subscription.request_id(), joining_start = %joining_start)
    )]
    /// Sends a Relative Joining FETCH for `subscription` (draft-14 §9.16.2):
    /// the range starts at object 0 of the group `joining_start` groups
    /// before the subscription's Largest Location. On a connection after the one
    /// the subscription was first sent on (the session followed a GOAWAY), the
    /// same range goes as a Standalone FETCH instead; there, a subscription with
    /// no Largest Location fails with INVALID_RANGE without sending a request.
    pub async fn fetch_relative_joining(
        &mut self,
        subscription: &Subscription,
        joining_start: u64,
        option: FetchOption,
    ) -> Result<FetchHandle, RequestFailure> {
        let (connection, joining_request_id) = self
            .on_live_connection(subscription.request_id())
            .unwrap_or_else(|| (self.session.connection(), subscription.request_id()));
        let remainder = joining_remainder(subscription, joining_start, &option);
        // A later connection holds the subscription with a `resumed_filter`, never
        // Largest Object, so a Joining FETCH may not join it.
        let fetch_params = if connection.generation == generation_of(subscription.request_id()) {
            FetchParams::RelativeJoining {
                joining_request_id,
                joining_start,
            }
        } else {
            let (ContentExists::True { location }, Some(remainder)) =
                (subscription.content_exists(), &remainder)
            else {
                return Err(RequestFailure::Rejected {
                    request_id: subscription.request_id(),
                    code: RequestErrorCode::InvalidRange,
                    reason: "no objects were published before the SUBSCRIBE".to_string(),
                });
            };
            FetchParams::Standalone {
                track_namespace: namespace_tuple(&remainder.track_namespace),
                track_name: remainder.track_name.clone(),
                start_location: remainder.start,
                end_location: location.next(),
            }
        };
        let track = (
            subscription.track_namespace().to_string(),
            subscription.track_name().to_string(),
        );
        self.send_fetch(connection, fetch_params, option, remainder, track)
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
        (track_namespace, track_name): (String, String),
    ) -> Result<FetchHandle, RequestFailure> {
        let sent = self
            .send_fetch_request(
                connection,
                fetch_params,
                &option,
                (track_namespace, track_name),
            )
            .await?;
        tracing::Span::current().record("request_id", sent.request_id);
        self.fetch_answered(sent, &option, remainder).await
    }

    async fn send_fetch_request(
        &self,
        connection: Arc<Connection>,
        fetch_params: FetchParams,
        option: &FetchOption,
        (track_namespace, track_name): (String, String),
    ) -> Result<SentFetch, RequestFailure> {
        let reservation = connection.control.reserve_request_id().await?;
        let request_id = reservation.request_id();
        let pending_fetch = self.session.register_pending_fetch(
            &connection,
            request_id,
            track_namespace,
            track_name,
        );
        let fetch = Fetch {
            request_id,
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order,
            fetch_params,
            authorization_tokens: vec![],
            fill_timeout: option
                .fill_timeout
                .map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)),
            trace_context: current_trace_context(&connection.control),
        };
        let response = connection
            .send_request(reservation, OutgoingRequest::Fetch(fetch))
            .await?;
        Ok(SentFetch {
            connection,
            request_id,
            pending_fetch,
            response,
        })
    }

    pub(crate) async fn fetch_answered(
        &self,
        sent: SentFetch,
        option: &FetchOption,
        remainder: Option<FetchRemainder>,
    ) -> Result<FetchHandle, RequestFailure> {
        let track_namespace = remainder
            .as_ref()
            .map(|remainder| remainder.track_namespace.as_str());
        let track_name = remainder
            .as_ref()
            .map(|remainder| remainder.track_name.as_str());
        let SentFetch {
            connection,
            pending_fetch,
            response,
            ..
        } = sent;
        let sent_at = response.sent_at();
        let response = response.response().await?.accept();
        match response {
            ResponseMessage::FetchOk(fetch_ok) => {
                tracing::info!(
                    request_id = fetch_ok.request_id,
                    track_namespace,
                    track_name,
                    duration_ms = elapsed_ms(sent_at),
                    "FETCH_OK received"
                );
                pending_fetch.keep();
                let mut handle = FetchHandle::new(FetchOk {
                    request_id: logical_id(connection.generation, fetch_ok.request_id),
                    ..fetch_ok
                });
                handle.remainder = remainder.filter(|_| {
                    self.session.follows_go_away() && option.group_order != GroupOrder::Descending
                });
                Ok(handle)
            }
            ResponseMessage::FetchError(request_id, error_code, reason_phrase) => {
                tracing::info!(
                    request_id,
                    track_namespace,
                    track_name,
                    ?error_code,
                    %reason_phrase,
                    duration_ms = elapsed_ms(sent_at),
                    "FETCH_ERROR received"
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

    /// Sends a SUBSCRIBE_UPDATE for `subscription` on each connection that
    /// carries it (draft-14 §9.10).
    /// The draft defines no response, so this returns once the message is
    /// written.
    #[tracing::instrument(
        level = "debug",
        name = "moqt.subscriber.subscribe_update",
        skip_all,
        fields(subscription_request_id = subscription.request_id(), forward = option.forward)
    )]
    pub async fn subscribe_update(
        &self,
        subscription: &Subscription,
        option: SubscribeUpdateOption,
    ) -> anyhow::Result<()> {
        let start = option.start_location;
        self.send_update(subscription, option, |_| start).await
    }

    /// `subscribe_update` with the Start Location `start_on` gives for each
    /// connection.
    async fn send_update(
        &self,
        subscription: &Subscription,
        option: SubscribeUpdateOption,
        start_on: impl Fn(&Connection) -> Location,
    ) -> anyhow::Result<()> {
        subscription.last_update().set(option);
        let carried = self.session.read_carried().await;
        if let Some(carried) = &carried {
            carried.update(|carried| {
                let Some(carried) = carried.subscriptions.get_mut(&subscription.request_id())
                else {
                    return;
                };
                carried.option.forward = option.forward;
                carried.option.subscriber_priority = option.subscriber_priority;
                if let Some(end_group) = option.end_group.checked_sub(1) {
                    carried.option.filter_type = FilterType::AbsoluteRange {
                        location: option.start_location,
                        end_group,
                    };
                }
            });
        }
        for (connection, subscription_request_id) in
            self.on_live_connections(subscription.request_id())
        {
            let reservation = connection.control.reserve_request_id().await?;
            let subscribe_update = SubscribeUpdate {
                request_id: reservation.request_id(),
                subscription_request_id,
                start_location: start_on(&connection),
                end_group: option.end_group,
                subscriber_priority: option.subscriber_priority,
                forward: option.forward,
                authorization_tokens: vec![],
                delivery_timeout: None,
                new_group_request: None,
            };
            connection
                .control
                .send_subscribe_update(reservation, subscribe_update)
                .await?;
        }
        Ok(())
    }

    /// Asks the Original Publisher to start a new group, with a REQUEST_UPDATE
    /// that carries NEW_GROUP_REQUEST (draft-18 §10.2.13). `largest_group` is
    /// the largest Group ID of the track the caller knows; a relay passes the
    /// request on only while the track's largest group is not beyond it.
    /// Returns false without sending for a track without DYNAMIC_GROUPS,
    /// which §10.2.13 forbids asking; no draft-14 track has it.
    #[tracing::instrument(
        level = "debug",
        name = "moqt.subscriber.request_new_group",
        skip_all,
        fields(subscription_request_id = subscription.request_id())
    )]
    pub async fn request_new_group(
        &self,
        subscription: &Subscription,
        largest_group: Option<u64>,
    ) -> anyhow::Result<bool> {
        if !subscription.track_properties().dynamic_groups() {
            return Ok(false);
        }
        let new_group_request = largest_group.map_or(0, |group_id| group_id + 1);
        let mut sent = false;
        for (connection, subscription_request_id) in
            self.on_live_connections(subscription.request_id())
        {
            let reservation = connection.control.reserve_request_id().await?;
            sent |= connection
                .control
                .send_new_group_request(reservation, subscription_request_id, new_group_request)
                .await?;
        }
        Ok(sent)
    }

    /// Flips Forward and repeats the subscription's current Start Location,
    /// End Group and priority, because a SUBSCRIBE_UPDATE may only narrow a
    /// subscription (draft-14 §9.10).
    pub async fn update_forward(
        &self,
        subscription: &Subscription,
        forward: bool,
    ) -> anyhow::Result<()> {
        self.repeat(
            subscription,
            SubscribeUpdateOption {
                forward,
                ..repeated_update(subscription)
            },
        )
        .await
    }

    /// Changes the Subscriber Priority (draft-14 §7.1) and repeats the
    /// subscription's current Start Location, End Group and Forward State.
    pub async fn update_priority(
        &self,
        subscription: &Subscription,
        subscriber_priority: u8,
    ) -> anyhow::Result<()> {
        self.repeat(
            subscription,
            SubscribeUpdateOption {
                subscriber_priority,
                ..repeated_update(subscription)
            },
        )
        .await
    }

    /// Ends the subscription before `group_id`: groups below it are still
    /// delivered and the publisher answers with PUBLISH_DONE once it has sent
    /// them (draft-14 §9.10, §9.12). The current Start Location, priority and
    /// Forward State are repeated.
    pub async fn end_before_group(
        &self,
        subscription: &Subscription,
        group_id: u64,
    ) -> anyhow::Result<()> {
        self.repeat(
            subscription,
            SubscribeUpdateOption {
                end_group: group_id,
                ..repeated_update(subscription)
            },
        )
        .await
    }

    /// Sends `option`, which repeats the subscription's Start Location, with
    /// the later start a SUBSCRIBE sent again after GOAWAY established on its
    /// connection.
    async fn repeat(
        &self,
        subscription: &Subscription,
        option: SubscribeUpdateOption,
    ) -> anyhow::Result<()> {
        let start = option.start_location;
        self.send_update(subscription, option, |connection| {
            connection
                .carried_start(subscription.request_id())
                .map_or(start, |carried| carried.max(start))
        })
        .await
    }

    #[tracing::instrument(
        level = "debug",
        name = "moqt.subscriber.unsubscribe",
        skip_all,
        fields(request_id = %subscription.request_id())
    )]
    pub async fn unsubscribe(&self, subscription: &Subscription) -> anyhow::Result<()> {
        if let Some(carried) = self.session.read_carried().await {
            carried.update(|carried| carried.subscriptions.remove(&subscription.request_id()));
        }
        for (connection, request_id) in self.on_live_connections(subscription.request_id()) {
            self.session
                .cancel_subscription(&connection, request_id)
                .await;
            connection
                .control
                .notify_end(Notification::Unsubscribe(Unsubscribe { request_id }))
                .await?;
        }
        Ok(())
    }

    /// Withdraws every namespace subscription for `namespace`, whatever its
    /// interest.
    #[tracing::instrument(
        level = "debug",
        name = "moqt.subscriber.unsubscribe_namespace",
        skip_all,
        fields(track_namespace_prefix = %namespace)
    )]
    pub async fn unsubscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        if let Some(carried) = self.session.read_carried().await {
            carried.update(|carried| {
                carried
                    .namespaces
                    .retain(|_, (carried, _)| *carried != namespace)
            });
        }
        let vec_namespace = namespace_tuple(&namespace);
        for connection in self.session.live_connections() {
            connection
                .control
                .notify_end(Notification::UnsubscribeNamespace {
                    track_namespace_prefix: vec_namespace.clone(),
                    interest: NamespaceInterest::NamespacesAndTracks,
                })
                .await?;
        }
        Ok(())
    }

    /// Withdraws the namespace subscription `interest` for `namespace` that
    /// one of the requests of a `subscribe_namespace` made.
    async fn withdraw_namespace_request(
        &self,
        namespace: &str,
        interest: NamespaceInterest,
    ) -> anyhow::Result<()> {
        if let Some(carried) = self.session.read_carried().await {
            carried.update(|carried| {
                carried.namespaces.retain(|_, (carried, carried_interest)| {
                    !(carried == namespace && *carried_interest == interest)
                })
            });
        }
        let vec_namespace = namespace_tuple(namespace);
        for connection in self.session.live_connections() {
            connection
                .control
                .notify_end(Notification::UnsubscribeNamespace {
                    track_namespace_prefix: vec_namespace.clone(),
                    interest,
                })
                .await?;
        }
        Ok(())
    }

    /// The objects of `subscription`, from its subgroup streams and its
    /// datagrams alike.
    pub fn track_reader(&self, subscription: &Subscription) -> TrackReader {
        TrackReader::of(
            Subscriber {
                session: self.session.clone(),
            },
            subscription.clone(),
        )
    }

    /// Subscribes with Largest Object and reads the group that object is in from
    /// its first object, through a Relative Joining FETCH with offset 0
    /// (draft-14 §9.16.2). The reader yields the fetched objects before the live
    /// ones. A track without content, or a FETCH that fails, yields the live
    /// objects only.
    pub async fn subscribe_current_group(
        &mut self,
        track_namespace: String,
        track_name: String,
    ) -> Result<(Subscription, TrackReader), RequestFailure> {
        let option = SubscribeOption {
            filter_type: FilterType::LargestObject,
            ..SubscribeOption::default()
        };
        let (subscription, joining_fetch) = self
            .subscribe_joining(track_namespace, track_name, option, true)
            .await?;
        let reader = TrackReader::joining_current_group(
            Subscriber {
                session: self.session.clone(),
            },
            subscription.clone(),
            joining_fetch,
        );
        Ok((subscription, reader))
    }

    /// Subscribes with Largest Object and reads the track from the first
    /// object of a group (draft-18 §5.1.3.1). The SUBSCRIBE_OK decides which
    /// group. For a track with dynamic groups (§12.6) whose Largest Object
    /// ID is `new_group_after` or more, the Original Publisher is asked for
    /// a new group (§10.2.13) and the reader yields from that group on; when
    /// none arrives within `NEW_GROUP_WAIT`, the current group is fetched
    /// after all. For any other track a Joining FETCH of the current group
    /// is sent, and the reader yields its objects before the live ones.
    /// `option`'s `filter_type`, `forward` and `new_group_request` are replaced
    /// by Largest Object, Forward 1 and none.
    pub async fn subscribe_group_start(
        &mut self,
        track_namespace: String,
        track_name: String,
        new_group_after: u64,
        option: SubscribeOption,
    ) -> Result<(Subscription, TrackReader), RequestFailure> {
        let option = SubscribeOption {
            filter_type: FilterType::LargestObject,
            forward: true,
            new_group_request: None,
            ..option
        };
        let (subscription, _) = self
            .subscribe_joining(track_namespace, track_name, option, false)
            .await?;
        let subscriber = Subscriber {
            session: self.session.clone(),
        };
        let requested = new_group_request(
            subscription.track_properties(),
            subscription.content_exists(),
            new_group_after,
        );
        let reader = match requested {
            Some(largest_group) => {
                if let Err(error) = self
                    .request_new_group(&subscription, Some(largest_group))
                    .await
                {
                    tracing::debug!(%error, "NEW_GROUP_REQUEST could not be sent");
                }
                TrackReader::awaiting_new_group(subscriber, subscription.clone(), largest_group)
            }
            None => TrackReader::fetching_current_group(subscriber, subscription.clone()),
        };
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
        level = "debug",
        name = "moqt.subscriber.accept_fetch_receiver",
        skip_all,
        fields(request_id = fetch_handle.request_id)
    )]
    pub async fn accept_fetch_receiver(
        &mut self,
        fetch_handle: &FetchHandle,
    ) -> anyhow::Result<FetchDataReceiver> {
        let request_id = fetch_handle.request_id;
        let mut channel = self
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
        let mut receiver = channel
            .receiver
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Failed to receive fetch stream"))?;
        receiver.set_group_order(fetch_handle.group_order);
        receiver.check_with(
            self.session
                .object_ledger(&channel.track_namespace, &channel.track_name),
        );
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

    /// Takes the receiver of `subscription`'s subgroup streams and datagrams.
    /// Fails when it was already taken or the subscription is not known.
    #[tracing::instrument(
        level = "debug",
        name = "moqt.subscriber.accept_data_receiver",
        skip_all,
        fields(track_alias = subscription.track_alias())
    )]
    pub async fn accept_data_receiver(
        &mut self,
        subscription: &Subscription,
    ) -> anyhow::Result<DataReceiver> {
        let track_alias = subscription.track_alias();
        let channel = self
            .session
            .receiver_map
            .lock()
            .await
            .remove(&track_alias)
            .ok_or_else(|| anyhow::anyhow!("No receiver for track_alias: {}", track_alias))?;
        Ok(DataReceiver::new(
            track_alias,
            channel.receiver,
            channel.version,
            self.session
                .object_ledger(subscription.track_namespace(), subscription.track_name()),
        ))
    }
}

/// The group a subscriber joining the track asks the publisher to end: the
/// largest one, when the track has dynamic groups (draft-18 §12.6) and that
/// group already holds more than `new_group_after` objects. `None` when
/// fetching the group's start is the better way in (§5.1.3.1), or the track
/// has no object.
fn new_group_request(
    track_properties: &TrackProperties,
    content_exists: ContentExists,
    new_group_after: u64,
) -> Option<u64> {
    match content_exists {
        ContentExists::True { location }
            if track_properties.dynamic_groups() && location.object_id >= new_group_after =>
        {
            Some(location.group_id)
        }
        _ => None,
    }
}

/// The last SUBSCRIBE_UPDATE sent for `subscription`, or before any update,
/// the range, priority and Forward State of its SUBSCRIBE or PUBLISH_OK.
/// `end_group` is the SUBSCRIBE_UPDATE field: the end group plus one, or 0
/// for an open-ended subscription.
fn repeated_update(subscription: &Subscription) -> SubscribeUpdateOption {
    if let Some(last) = subscription.last_update().get() {
        return last;
    }
    let largest = match subscription.content_exists() {
        ContentExists::True { location } => Some(location),
        ContentExists::False => None,
    };
    let filter_type = subscription.filter_type();
    SubscribeUpdateOption {
        start_location: filter_type.start_location(largest),
        end_group: match filter_type {
            FilterType::AbsoluteRange { end_group, .. } => end_group + 1,
            _ => 0,
        },
        subscriber_priority: subscription.subscriber_priority(),
        forward: subscription.forward(),
    }
}

/// One namespace request, of an `interest` `ControlChannel::namespace_requests`
/// returned for `connection`.
pub(crate) async fn send_subscribe_namespace(
    connection: &Arc<Connection>,
    namespace: String,
    interest: NamespaceInterest,
) -> Result<RequestId, RequestFailure> {
    let (request_id, pending) = begin_subscribe_namespace(connection, &namespace, interest).await?;
    subscribe_namespace_ok(request_id, &namespace, pending).await
}

async fn begin_subscribe_namespace(
    connection: &Arc<Connection>,
    namespace: &str,
    interest: NamespaceInterest,
) -> Result<(RequestId, PendingResponse), RequestFailure> {
    let vec_namespace = namespace_tuple(namespace);
    let reservation = connection.control.reserve_request_id().await?;
    let request_id = reservation.request_id();
    let message = SubscribeNamespace {
        trace_context: current_trace_context(&connection.control),
        ..SubscribeNamespace::new(request_id, vec_namespace, vec![])
    };
    let request = match interest {
        NamespaceInterest::Tracks => OutgoingRequest::SubscribeTracks(message),
        NamespaceInterest::Namespaces | NamespaceInterest::NamespacesAndTracks => {
            OutgoingRequest::SubscribeNamespace(message)
        }
    };
    let pending = connection.send_request(reservation, request).await?;
    Ok((request_id, pending))
}

async fn subscribe_namespace_ok(
    request_id: RequestId,
    namespace: &str,
    pending: PendingResponse,
) -> Result<RequestId, RequestFailure> {
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
                Ok(request_id)
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

/// A FETCH written to the peer; `Subscriber::fetch_answered` reads its answer.
pub(crate) struct SentFetch {
    connection: Arc<Connection>,
    request_id: RequestId,
    pending_fetch: PendingFetch,
    response: PendingResponse,
}

/// What a session that follows GOAWAY asks the next connection for when the
/// response stream of a Joining FETCH of `subscription` is cut off.
pub(crate) fn joining_remainder(
    subscription: &Subscription,
    joining_start: u64,
    option: &FetchOption,
) -> Option<FetchRemainder> {
    match subscription.content_exists() {
        ContentExists::True { location } => Some(FetchRemainder::new(
            subscription.track_namespace().to_string(),
            subscription.track_name().to_string(),
            Location {
                group_id: location.group_id.saturating_sub(joining_start),
                object_id: 0,
            },
            option,
        )),
        ContentExists::False => None,
    }
}

/// The SUBSCRIBE_OK, with `connection`'s wire ids, and its
/// `ReceivedResponse`: dropping that before `accept` sends UNSUBSCRIBE.
pub(crate) async fn send_subscribe(
    connection: &Arc<Connection>,
    track_namespace: &str,
    track_name: &str,
    option: &SubscribeOption,
) -> Result<(SubscribeOk, ReceivedResponse), RequestFailure> {
    let sent = send_subscribe_request(connection, track_namespace, track_name, option).await?;
    subscribe_ok(sent, track_namespace, track_name).await
}

/// A SUBSCRIBE written to the peer; `subscribe_ok` reads its answer.
struct SentSubscribe {
    request_id: RequestId,
    pending: PendingResponse,
}

async fn send_subscribe_request(
    connection: &Arc<Connection>,
    track_namespace: &str,
    track_name: &str,
    option: &SubscribeOption,
) -> Result<SentSubscribe, RequestFailure> {
    let reservation = connection.control.reserve_request_id().await?;
    let request_id = reservation.request_id();
    let subscribe = Subscribe {
        request_id,
        track_namespace: namespace_tuple(track_namespace),
        track_name: track_name.to_string(),
        subscriber_priority: option.subscriber_priority,
        group_order: option.group_order,
        forward: option.forward,
        filter_type: option.filter_type,
        authorization_tokens: vec![],
        delivery_timeout: option.delivery_timeout,
        new_group_request: option.new_group_request,
        trace_context: current_trace_context(&connection.control),
    };
    let pending = connection
        .send_request(reservation, OutgoingRequest::Subscribe(subscribe))
        .await?;
    Ok(SentSubscribe {
        request_id,
        pending,
    })
}

async fn subscribe_ok(
    SentSubscribe {
        request_id,
        pending,
    }: SentSubscribe,
    track_namespace: &str,
    track_name: &str,
) -> Result<(SubscribeOk, ReceivedResponse), RequestFailure> {
    let sent_at = pending.sent_at();
    let response = pending.response().await?;
    if let ResponseMessage::SubscribeOk(message) = response.message()
        && message.request_id == request_id
    {
        tracing::info!(
            request_id,
            track_namespace,
            track_name,
            track_alias = message.track_alias,
            duration_ms = elapsed_ms(sent_at),
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
                duration_ms = elapsed_ms(sent_at),
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
    use std::time::Duration;

    use bytes::Bytes;
    use futures::FutureExt;

    use crate::namespace::NamespaceInterest;

    use tokio::task::JoinHandle;

    use crate::{
        RequestErrorCode, RequestFailure, Session, SessionEvent, Subscription,
        TerminationErrorCode, TrackReader, Version,
        fetch::{
            Fetch, FetchCancelHandler, FetchDataReceiver, FetchDataSender, FetchHandle,
            FetchHandler, FetchOption, FetchParams,
        },
        modules::{
            moqt::domains::connection::IncomingObjectNotification,
            test_support::{
                DISCONNECT_TIMEOUT, DRAFTS, HANDSHAKE_TIMEOUT, capture_telemetry,
                connected_sessions, data_receiver, first_subgroup_stream, location, next_subgroup,
                notify_datagram, object_with_extensions, read_fetch_objects, read_subgroup_objects,
                receive_disconnected, send_subgroup_object, send_until_stopped,
                send_without_waiting, spawn_connected_draft18_sessions,
                spawn_connected_native_sessions, spawn_connected_native_sessions_following_go_away,
                subscribe_namespace_accepted, subscribed_track, wait_for_control_delivery, within,
            },
            transport::transport_send_stream::TransportSendError,
        },
        object::{
            ContentExists, ExtensionHeaders, ForwardingPreference, Location, Object, ObjectStatus,
        },
        subscribe::{
            FilterType, SubscribeHandler, SubscribeOption, SubscribeUpdateHandler,
            SubscribeUpdateOption,
        },
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
    /// stream that is never finished or stopped stalls a later round.
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
    async fn open_fetch(
        client: &Session,
        server: &Session,
        track_name: &str,
        first: &Object,
    ) -> OpenFetch {
        let (request, handler) = start_fetch(client, server, track_name).await;
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

    fn holds_ledger(client: &Session) -> bool {
        client.subscriber().session.holds_ledger("ns", TRACK)
    }

    async fn fetch_never_taken_keeps_no_ledger(client: Session, server: Session) {
        // Arrange
        let (request, handler) = start_fetch(&client, &server, TRACK).await;
        // Act: the response ends while the application never takes it
        serve_fetch(&server, &handler, END, &[fetch_object(START, b"object")]).await;
        let _handle = request.await.unwrap().unwrap();
        // Assert
        assert!(!holds_ledger(&client));
    }

    #[tokio::test]
    async fn a_fetch_whose_response_is_never_taken_keeps_no_ledger() {
        for version in DRAFTS {
            let (client, server) =
                connected_sessions("subscriber-fetch-ledger-untaken", version).await;
            fetch_never_taken_keeps_no_ledger(client, server).await;
        }
    }

    async fn fetch_releases_the_ledger_at_its_end(client: Session, server: Session) {
        // Arrange
        let (request, handler) = start_fetch(&client, &server, TRACK).await;
        serve_fetch(&server, &handler, END, &[fetch_object(START, b"object")]).await;
        let handle = request.await.unwrap().unwrap();
        let mut receiver = client
            .subscriber()
            .accept_fetch_receiver(&handle)
            .await
            .unwrap();
        let held_while_read = holds_ledger(&client);
        // Act: read up to the FIN, keeping the receiver
        within(read_fetch_objects(&mut receiver)).await;
        // Assert: draft-18 §5.2, the FETCH state ends with the FIN
        assert!(held_while_read);
        assert!(!holds_ledger(&client));
    }

    #[tokio::test]
    async fn a_fetch_response_read_to_its_end_releases_the_ledger() {
        for version in DRAFTS {
            let (client, server) = connected_sessions("subscriber-fetch-ledger-end", version).await;
            fetch_releases_the_ledger_at_its_end(client, server).await;
        }
    }

    #[tokio::test]
    async fn accepted_fetch_releases_its_stream_sender_and_keeps_delivering_objects() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-fetch-sender", version).await;
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
    }

    #[tokio::test]
    async fn fetch_receiver_fails_when_the_peer_disconnects_before_the_stream() {
        for version in DRAFTS {
            // Arrange
            let (client, server) =
                connected_sessions("subscriber-fetch-disconnect-before-stream", version).await;
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
    }

    #[tokio::test]
    async fn fetch_ok_hands_its_max_cache_duration_to_the_fetch_handle() {
        for version in DRAFTS {
            // Arrange
            let (client, server) =
                connected_sessions("subscriber-fetch-max-cache-duration", version).await;
            let (request, mut handler) = start_fetch(&client, &server, TRACK).await;
            handler.track_properties.set_max_cache_duration(Some(2_000));

            // Act
            handler.ok(false, END).await.unwrap();

            // Assert
            let fetch_handle = request.await.unwrap().unwrap();
            assert_eq!(
                fetch_handle.track_properties.max_cache_duration(),
                Some(2_000)
            );
        }
    }

    #[tokio::test]
    async fn rejected_fetch_releases_its_pending_state() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-fetch-rejected", version).await;
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
    }

    #[tokio::test]
    async fn abandoned_fetch_releases_its_pending_state() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-fetch-abandoned", version).await;
            let (request, handler) = start_fetch(&client, &server, TRACK).await;

            // Act
            request.abort();
            let _ = request.await;

            // Assert
            assert!(!has_pending_fetch(&client, handler.request_id));
        }
    }

    #[tokio::test]
    async fn repeated_fetches_on_one_session_each_end_and_release_their_state() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-fetch-repeated", version).await;
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
                let completed = tokio::time::timeout(
                    HANDSHAKE_TIMEOUT,
                    complete_fetch(&client, &server, objects),
                )
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
    }

    #[tokio::test]
    async fn fetch_ok_after_the_caller_stopped_waiting_cancels_the_fetch_and_later_fetches_succeed()
    {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-fetch-late-ok", version).await;
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
            assert!(failed_for_the_cancel(version, &stopped), "{stopped:#}");
            assert!(!has_pending_fetch(&client, handler.request_id));
            assert_eq!(later, later_objects);
        }
    }

    #[tokio::test]
    async fn fetches_answered_in_reverse_order_each_get_their_own_response_and_objects() {
        for version in DRAFTS {
            // Arrange
            let (client, server) =
                connected_sessions("subscriber-fetch-reverse-order", version).await;
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
    }

    #[tokio::test]
    async fn cancelling_one_fetch_leaves_the_other_fetches_on_the_session_running() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-fetch-cancel-one", version).await;
            let firsts = [b"a0", b"b0", b"c0"].map(|payload| fetch_object(location(0, 0), payload));
            let seconds = [b"a1", b"c1"].map(|payload| fetch_object(location(0, 1), payload));
            let mut kept_a = open_fetch(&client, &server, "track-a", &firsts[0]).await;
            let cancelled = open_fetch(&client, &server, "track-b", &firsts[1]).await;
            let mut kept_c = open_fetch(&client, &server, "track-c", &firsts[2]).await;

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
            assert!(failed_for_the_cancel(version, &stopped), "{stopped:#}");
            assert_eq!(received_a, [firsts[0].clone(), seconds[0].clone()]);
            assert_eq!(received_c, [firsts[2].clone(), seconds[1].clone()]);
        }
    }

    #[tokio::test]
    async fn fetch_fails_as_soon_as_the_peer_disconnects_without_answering() {
        for version in DRAFTS {
            // Arrange
            let (client, server) =
                connected_sessions("subscriber-fetch-disconnect-response", version).await;
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
    }

    #[tokio::test]
    async fn fetch_stream_fails_instead_of_ending_when_the_peer_disconnects_mid_stream() {
        for version in DRAFTS {
            // Arrange
            let (client, server) =
                connected_sessions("subscriber-fetch-disconnect-mid-stream", version).await;
            let mut open =
                open_fetch(&client, &server, TRACK, &fetch_object(START, b"first")).await;
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
    }

    async fn start_subscribe(
        client: &Session,
        server: &Session,
        track_name: &str,
    ) -> (
        JoinHandle<Result<Subscription, RequestFailure>>,
        SubscribeHandler,
    ) {
        start_subscribe_with(client, server, track_name, SubscribeOption::default()).await
    }

    async fn start_subscribe_with(
        client: &Session,
        server: &Session,
        track_name: &str,
        option: SubscribeOption,
    ) -> (
        JoinHandle<Result<Subscription, RequestFailure>>,
        SubscribeHandler,
    ) {
        let mut subscriber = client.subscriber();
        let track_name = track_name.to_string();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe("ns".to_string(), track_name, option)
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
        for version in DRAFTS {
            // Arrange
            let (client, server) =
                connected_sessions("subscriber-subscribe-late-ok", version).await;
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
                send_subgroup_object(&server, &later_handler.into_subscription(alias), &object)
                    .await;
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
    }

    #[tokio::test]
    async fn unsubscribing_one_track_leaves_the_other_delivering_on_the_same_session() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-unsubscribe-one", version).await;
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
            let (_, cancelled_before) =
                within(receive_subscribe_objects(&client, &cancelled)).await;
            within(send_subgroup_object(
                &server,
                &kept_handler.into_subscription(kept_alias),
                &before,
            ))
            .await;
            let mut kept_receiver = within(data_receiver(&client, &kept)).await;
            let mut first = within(next_subgroup(&mut kept_receiver)).await;
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
                let mut next = next_subgroup(&mut kept_receiver).await;
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
    }

    #[tokio::test]
    async fn subscribes_answered_in_reverse_order_receive_their_own_metadata_alias_and_objects() {
        for version in DRAFTS {
            // Arrange
            let (client, server) =
                connected_sessions("subscriber-subscribe-reverse-order", version).await;
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
                    handler
                        .track_properties
                        .set_max_cache_duration(Some(1000 + index as u64));
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
                assert_eq!(
                    subscription.track_properties().max_cache_duration(),
                    Some(1000 + index as u64)
                );
                assert_eq!(received_objects, [objects[index].clone()]);
            }
            assert_ne!(aliases[0], aliases[1]);
            assert_ne!(aliases[1], aliases[2]);
            assert_ne!(aliases[0], aliases[2]);
        }
    }

    /// A LargestObject subscription whose SUBSCRIBE_OK reported Largest
    /// Location {3, 1}, so its Start Location is {3, 2}.
    async fn subscribe_after_group_3(
        client: &Session,
        server: &Session,
    ) -> (Subscription, SubscribeHandler) {
        let (request, handler) = start_subscribe(client, server, "track").await;
        let largest = Location {
            group_id: 3,
            object_id: 1,
        };
        handler
            .ok(0, ContentExists::True { location: largest })
            .await
            .unwrap();
        (request.await.unwrap().unwrap(), handler)
    }

    async fn receive_subscribe_update(server: &Session) -> SubscribeUpdateHandler {
        let SessionEvent::SubscribeUpdate(update) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE_UPDATE from the client");
        };
        update
    }

    /// A subscription to `ns/track` the server answered with Largest {3, 1}.
    async fn subscribed(client: &Session, server: &Session) -> (Subscription, SubscribeHandler) {
        let (request, handler) = start_subscribe(client, server, "track").await;
        let largest = location(3, 1);
        handler
            .ok(0, ContentExists::True { location: largest })
            .await
            .unwrap();
        (request.await.unwrap().unwrap(), handler)
    }

    /// How sending on the response stream of a FETCH the subscriber
    /// cancelled fails: draft-14 meets the subscriber's STOP_SENDING, while
    /// a draft-18 publisher resets the stream itself (§5.2).
    fn failed_for_the_cancel(version: Version, error: &anyhow::Error) -> bool {
        match version {
            Version::Draft14 => TransportSendError::is_stopped_by_peer(error),
            Version::Draft18 => error.chain().any(|cause| {
                matches!(
                    cause.downcast_ref::<TransportSendError>(),
                    Some(TransportSendError::ClosedStream)
                )
            }),
        }
    }

    #[tokio::test]
    async fn a_relative_joining_fetch_names_its_subscription_and_delivers_the_joined_objects() {
        for version in DRAFTS {
            // Arrange: the subscription's Largest is {3, 1}
            let (client, server) = connected_sessions("subscriber-relative-joining", version).await;
            let (subscription, subscribe_handler) = subscribed(&client, &server).await;
            let joined = [
                fetch_object(location(2, 0), b"joined-2"),
                fetch_object(location(3, 0), b"joined-3"),
            ];

            // Act
            let mut subscriber = client.subscriber();
            let (handle, fetch_params) = within(async {
                tokio::join!(
                    subscriber.fetch_relative_joining(&subscription, 1, FetchOption::default()),
                    async {
                        let SessionEvent::Fetch(handler) = server.receive_event().await.unwrap()
                        else {
                            panic!("expected FETCH from the client");
                        };
                        serve_fetch(&server, &handler, location(3, 2), &joined).await;
                        handler.fetch.fetch_params.clone()
                    }
                )
            })
            .await;
            let mut receiver = subscriber
                .accept_fetch_receiver(&handle.unwrap())
                .await
                .unwrap();
            let fetched = within(read_fetch_objects(&mut receiver)).await;

            // Assert
            assert_eq!(
                fetch_params,
                FetchParams::RelativeJoining {
                    joining_request_id: subscribe_handler.request_id(),
                    joining_start: 1,
                }
            );
            assert_eq!(fetched, joined);
        }
    }

    #[tokio::test]
    async fn update_forward_repeats_the_subscription_fields_with_the_new_forward_flag() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-update-forward", version).await;
            let (subscription, handler) = subscribed(&client, &server).await;

            // Act
            client
                .subscriber()
                .update_forward(&subscription, false)
                .await
                .unwrap();

            // Assert
            let SessionEvent::SubscribeUpdate(update) = server.receive_event().await.unwrap()
            else {
                panic!("expected SUBSCRIBE_UPDATE from the client");
            };
            assert_eq!(update.subscription_request_id(), handler.request_id());
            assert!(!update.forward());
            assert_eq!(
                update.filter(),
                Some(FilterType::AbsoluteStart {
                    location: Location {
                        group_id: 3,
                        object_id: 2
                    }
                })
            );
            assert_eq!(update.subscriber_priority(), 128);
        }
    }

    #[tokio::test]
    async fn update_priority_repeats_the_subscription_fields_with_the_new_priority() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-update-priority", version).await;
            let (subscription, handler) = subscribed(&client, &server).await;

            // Act
            client
                .subscriber()
                .update_priority(&subscription, 7)
                .await
                .unwrap();

            // Assert
            let SessionEvent::SubscribeUpdate(update) = server.receive_event().await.unwrap()
            else {
                panic!("expected SUBSCRIBE_UPDATE from the client");
            };
            assert_eq!(update.subscription_request_id(), handler.request_id());
            assert_eq!(update.subscriber_priority(), 7);
            assert!(update.forward());
            assert_eq!(
                update.filter(),
                Some(FilterType::AbsoluteStart {
                    location: Location {
                        group_id: 3,
                        object_id: 2
                    }
                })
            );
        }
    }

    #[tokio::test]
    async fn update_forward_after_update_priority_repeats_that_priority() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions("subscriber-update-forward-after-priority").await;
        let (subscription, _handler) = subscribe_after_group_3(&client, &server).await;
        client
            .subscriber()
            .update_priority(&subscription, 7)
            .await
            .unwrap();
        receive_subscribe_update(&server).await;

        // Act
        client
            .subscriber()
            .update_forward(&subscription, false)
            .await
            .unwrap();

        // Assert: repeating the SUBSCRIBE's priority would undo the update
        let update = receive_subscribe_update(&server).await;
        assert!(!update.forward());
        assert_eq!(update.subscriber_priority(), 7);
    }

    #[tokio::test]
    async fn accepting_an_update_tells_the_subscriber_the_largest_object_only_on_draft18() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-update-largest", version).await;
            let (subscription, _handler) = subscribed(&client, &server).await;
            client
                .subscriber()
                .update_forward(&subscription, true)
                .await
                .unwrap();
            let SessionEvent::SubscribeUpdate(update) = server.receive_event().await.unwrap()
            else {
                panic!("expected SUBSCRIBE_UPDATE from the client");
            };

            // Act
            let told = update.ok(Some(location(3, 1)));

            // Assert: draft-18 §10.2.11 REQUEST_OK; draft-14 SUBSCRIBE_UPDATE has no answer
            assert_eq!(
                told,
                crate::subscribe::UpdateAnswer::Accepted {
                    largest_told: version == Version::Draft18
                },
                "{version:?}"
            );
        }
    }

    #[tokio::test]
    async fn end_before_group_sends_the_group_as_end_group_plus_one() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-end-before-group", version).await;
            let (subscription, handler) = subscribed(&client, &server).await;

            // Act
            client
                .subscriber()
                .end_before_group(&subscription, 7)
                .await
                .unwrap();

            // Assert: groups up to 6 stay in the range, which the field encodes as 6 + 1
            let SessionEvent::SubscribeUpdate(update) = server.receive_event().await.unwrap()
            else {
                panic!("expected SUBSCRIBE_UPDATE from the client");
            };
            assert_eq!(update.subscription_request_id(), handler.request_id());
            assert!(update.forward());
            assert_eq!(
                update.filter(),
                Some(FilterType::AbsoluteRange {
                    location: Location {
                        group_id: 3,
                        object_id: 2
                    },
                    end_group: 6,
                })
            );
        }
    }

    #[tokio::test]
    async fn end_before_group_repeats_the_forward_state_of_the_subscribe() {
        for version in DRAFTS {
            // Arrange
            let (client, server) =
                connected_sessions("subscriber-end-before-group-forward", version).await;
            let option = SubscribeOption {
                forward: false,
                ..SubscribeOption::default()
            };
            let (request, handler) = start_subscribe_with(&client, &server, "track", option).await;
            handler.ok(0, ContentExists::False).await.unwrap();
            let subscription = request.await.unwrap().unwrap();

            // Act
            client
                .subscriber()
                .end_before_group(&subscription, 7)
                .await
                .unwrap();

            // Assert: Forward 1 would start the delivery the SUBSCRIBE held back
            assert!(!receive_subscribe_update(&server).await.forward());
        }
    }

    #[tokio::test]
    async fn end_before_group_after_update_forward_repeats_that_forward_state() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions("subscriber-end-after-update-forward").await;
        let (subscription, _handler) = subscribe_after_group_3(&client, &server).await;
        client
            .subscriber()
            .update_forward(&subscription, false)
            .await
            .unwrap();
        receive_subscribe_update(&server).await;

        // Act
        client
            .subscriber()
            .end_before_group(&subscription, 7)
            .await
            .unwrap();

        // Assert
        assert!(!receive_subscribe_update(&server).await.forward());
    }

    #[tokio::test]
    async fn update_forward_after_end_before_group_repeats_that_end_group() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions("subscriber-update-forward-after-end").await;
        let (subscription, _handler) = subscribe_after_group_3(&client, &server).await;
        client
            .subscriber()
            .end_before_group(&subscription.clone(), 7)
            .await
            .unwrap();
        receive_subscribe_update(&server).await;

        // Act
        client
            .subscriber()
            .update_forward(&subscription, false)
            .await
            .unwrap();

        // Assert: an End Group of 0 would widen the range back to open-ended
        let update = receive_subscribe_update(&server).await;
        assert!(!update.forward());
        assert_eq!(
            update.filter(),
            Some(FilterType::AbsoluteRange {
                location: Location {
                    group_id: 3,
                    object_id: 2
                },
                end_group: 6,
            })
        );
    }

    #[tokio::test]
    async fn update_forward_after_subscribe_update_repeats_its_start_location_and_priority() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions("subscriber-update-forward-after-update").await;
        let (subscription, _handler) = subscribe_after_group_3(&client, &server).await;
        let narrowed_start = Location {
            group_id: 5,
            object_id: 0,
        };
        client
            .subscriber()
            .subscribe_update(
                &subscription.clone(),
                SubscribeUpdateOption {
                    start_location: narrowed_start,
                    end_group: 0,
                    subscriber_priority: 9,
                    forward: true,
                },
            )
            .await
            .unwrap();
        receive_subscribe_update(&server).await;

        // Act
        client
            .subscriber()
            .update_forward(&subscription, false)
            .await
            .unwrap();

        // Assert: the SUBSCRIBE's Start Location {3, 2} would move the start back
        let update = receive_subscribe_update(&server).await;
        assert!(!update.forward());
        assert_eq!(
            update.filter(),
            Some(FilterType::AbsoluteStart {
                location: narrowed_start
            })
        );
        assert_eq!(update.subscriber_priority(), 9);
    }

    #[tokio::test]
    async fn subscribe_update_sends_the_given_fields_for_the_given_subscription() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-subscribe-update", version).await;
            let option = SubscribeUpdateOption {
                start_location: Location {
                    group_id: 4,
                    object_id: 2,
                },
                end_group: 9,
                subscriber_priority: 7,
                forward: true,
            };

            let (subscription, handler) = subscribed(&client, &server).await;

            // Act
            client
                .subscriber()
                .subscribe_update(&subscription, option)
                .await
                .unwrap();

            // Assert
            let SessionEvent::SubscribeUpdate(update) = server.receive_event().await.unwrap()
            else {
                panic!("expected SUBSCRIBE_UPDATE from the client");
            };
            assert_eq!(update.subscription_request_id(), handler.request_id());
            assert_eq!(
                update.filter(),
                Some(FilterType::AbsoluteRange {
                    location: Location {
                        group_id: 4,
                        object_id: 2
                    },
                    end_group: 8,
                })
            );
            assert_eq!(update.subscriber_priority(), 7);
            assert!(update.forward());
        }
    }

    #[tokio::test]
    async fn fetch_dropped_after_its_fetch_ok_arrived_cancels_the_fetch() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions("subscriber-fetch-drop-after-ok").await;
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
    async fn unsubscribe_after_the_session_ended_succeeds() {
        for version in DRAFTS {
            // Arrange
            let track = subscribed_track("subscriber-unsubscribe-after-disconnect", version).await;
            track
                .server
                .close_with_error(TerminationErrorCode::NoError, "closing");
            receive_disconnected(&track.client).await.unwrap();

            // Act
            let unsubscribed = track
                .client
                .subscriber()
                .unsubscribe(&track.subscription)
                .await;

            // Assert
            assert!(unsubscribed.is_ok(), "{version:?}: {unsubscribed:?}");
        }
    }

    #[tokio::test]
    async fn subscribe_dropped_after_its_subscribe_ok_arrived_unsubscribes() {
        // Arrange
        let (client, server) = spawn_connected_native_sessions("subscriber-drop-after-ok").await;
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
        let (client, server) = spawn_connected_native_sessions("subscriber-drop-registering").await;
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
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscriber-peer-disconnects", version).await;
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

    const NEW_GROUP_AFTER: u64 = 5;
    const SUBSCRIBER_PRIORITY: u8 = 7;

    /// The reader of a `subscribe_group_start` the server accepted with
    /// `content_exists`, and the server's side of the subscription.
    async fn start_subscribe_group_start(
        client: &Session,
        server: &Session,
        content_exists: ContentExists,
        dynamic_groups: bool,
    ) -> (TrackReader, Subscription) {
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe_group_start(
                    "ns".to_string(),
                    TRACK.to_string(),
                    NEW_GROUP_AFTER,
                    SubscribeOption {
                        subscriber_priority: SUBSCRIBER_PRIORITY,
                        ..SubscribeOption::default()
                    },
                )
                .await
        });
        let SessionEvent::Subscribe(mut handler) = within(server.receive_event()).await.unwrap()
        else {
            panic!("expected SUBSCRIBE from the client");
        };
        handler.track_properties.set_dynamic_groups(dynamic_groups);
        let alias = handler.ok(0, content_exists).await.unwrap();
        let (_, reader) = request.await.unwrap().unwrap();
        (reader, handler.into_subscription(alias))
    }

    fn largest(group_id: u64, object_id: u64) -> ContentExists {
        ContentExists::True {
            location: location(group_id, object_id),
        }
    }

    async fn receives_no_event(server: &Session) -> bool {
        tokio::time::timeout(Duration::from_millis(100), server.receive_event())
            .await
            .is_err()
    }

    #[tokio::test]
    async fn subscribe_sends_its_delivery_timeout() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("subscriber-delivery-timeout", "moqt").await;
        let mut subscriber = client.subscriber();
        let option = SubscribeOption {
            delivery_timeout: Some(2_000),
            ..SubscribeOption::default()
        };

        // Act
        let request = tokio::spawn(async move {
            subscriber
                .subscribe("ns".to_string(), TRACK.to_string(), option)
                .await
        });
        let SessionEvent::Subscribe(handler) = within(server.receive_event()).await.unwrap() else {
            panic!("expected SUBSCRIBE from the client");
        };

        // Assert
        assert_eq!(handler.delivery_timeout, Some(2_000));
        request.abort();
    }

    #[tokio::test]
    async fn subscribe_group_start_sends_its_delivery_timeout() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("subscriber-group-start-timeout", "moqt").await;
        let mut subscriber = client.subscriber();

        // Act
        let request = tokio::spawn(async move {
            subscriber
                .subscribe_group_start(
                    "ns".to_string(),
                    TRACK.to_string(),
                    NEW_GROUP_AFTER,
                    SubscribeOption {
                        delivery_timeout: Some(2_000),
                        ..SubscribeOption::default()
                    },
                )
                .await
        });
        let SessionEvent::Subscribe(handler) = within(server.receive_event()).await.unwrap() else {
            panic!("expected SUBSCRIBE from the client");
        };

        // Assert
        assert_eq!(handler.delivery_timeout, Some(2_000));
        request.abort();
    }

    #[tokio::test]
    async fn subscribe_group_start_sends_its_subscriber_priority() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("subscriber-group-start-priority", "moqt").await;

        // Act
        let (_reader, served) =
            start_subscribe_group_start(&client, &server, largest(3, 1), true).await;

        // Assert
        assert_eq!(served.subscriber_priority(), SUBSCRIBER_PRIORITY);
    }

    #[tokio::test]
    async fn subscribe_group_start_fetches_a_group_that_began_recently() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("subscriber-group-start-recent", "moqt").await;

        // Act
        let (mut reader, served) =
            start_subscribe_group_start(&client, &server, largest(3, 1), true).await;
        let event = within(server.receive_event()).await.unwrap();
        let SessionEvent::Fetch(fetch) = event else {
            panic!("expected the joining FETCH, got {event:?}");
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
        send_subgroup_object(&server, &served, &fetch_object(location(3, 2), b"live")).await;
        let payloads = read_payloads(&mut reader, 3).await;

        // Assert
        assert_eq!(payloads, ["fetched-0", "fetched-1", "live"]);
    }

    #[tokio::test]
    async fn subscribe_group_start_fetches_an_old_group_of_a_track_without_dynamic_groups() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("subscriber-group-start-static", "moqt").await;

        // Act
        let _subscribed =
            start_subscribe_group_start(&client, &server, largest(3, NEW_GROUP_AFTER), false).await;
        let event = within(server.receive_event()).await.unwrap();

        // Assert: §10.2.13 forbids NEW_GROUP_REQUEST without DYNAMIC_GROUPS
        assert!(matches!(event, SessionEvent::Fetch(_)), "{event:?}");
    }

    #[tokio::test]
    async fn subscribe_group_start_reads_a_track_without_content_without_a_fetch() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("subscriber-group-start-empty", "moqt").await;

        // Act
        let (mut reader, served) =
            start_subscribe_group_start(&client, &server, ContentExists::False, true).await;
        let nothing_requested = receives_no_event(&server).await;
        send_subgroup_object(&server, &served, &fetch_object(location(3, 0), b"live")).await;
        let payloads = read_payloads(&mut reader, 1).await;

        // Assert
        assert!(nothing_requested);
        assert_eq!(payloads, ["live"]);
    }

    /// The REQUEST_UPDATE the client sends next, with no FETCH before it.
    async fn next_request_update(server: &Session) -> SubscribeUpdateHandler {
        match within(server.receive_event()).await.unwrap() {
            SessionEvent::SubscribeUpdate(update) => update,
            event => panic!("expected REQUEST_UPDATE, got {event:?}"),
        }
    }

    #[tokio::test]
    async fn subscribe_group_start_asks_for_a_new_group_and_yields_from_it() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("subscriber-group-start-new", "moqt").await;

        // Act
        let (mut reader, served) =
            start_subscribe_group_start(&client, &server, largest(3, NEW_GROUP_AFTER), true).await;
        let update = next_request_update(&server).await;
        send_subgroup_object(&server, &served, &fetch_object(location(3, 6), b"rest")).await;
        send_subgroup_object(&server, &served, &fetch_object(location(4, 0), b"new")).await;
        let payloads = read_payloads(&mut reader, 1).await;

        // Assert: the rest of group 3 cannot be decoded and is not yielded
        assert_eq!(update.new_group_request(), Some(4));
        assert_eq!(update.filter(), None);
        assert_eq!(payloads, ["new"]);
    }

    #[tokio::test]
    async fn subscribe_group_start_fetches_the_current_group_when_no_new_group_comes() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("subscriber-group-start-wait", "moqt").await;
        let (mut reader, served) =
            start_subscribe_group_start(&client, &server, largest(3, NEW_GROUP_AFTER), true).await;
        next_request_update(&server).await;
        send_subgroup_object(&server, &served, &fetch_object(location(3, 6), b"rest")).await;

        // Act: the publisher starts no new group
        let SessionEvent::Fetch(fetch) = within(server.receive_event()).await.unwrap() else {
            panic!("expected the joining FETCH");
        };
        serve_fetch(
            &server,
            &fetch,
            location(3, 6),
            &[fetch_object(location(3, 0), b"fetched")],
        )
        .await;
        let payloads = read_payloads(&mut reader, 2).await;

        // Assert
        assert_eq!(payloads, ["fetched", "rest"]);
    }

    #[tokio::test]
    async fn subscribe_group_start_sends_the_joining_fetch_after_subscribe_ok() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("subscriber-group-start-ordered", "moqt").await;
        let mut subscriber = client.subscriber();
        let _request = tokio::spawn(async move {
            subscriber
                .subscribe_group_start(
                    "ns".to_string(),
                    TRACK.to_string(),
                    NEW_GROUP_AFTER,
                    SubscribeOption {
                        subscriber_priority: SUBSCRIBER_PRIORITY,
                        ..SubscribeOption::default()
                    },
                )
                .await
        });
        let SessionEvent::Subscribe(subscribe) = within(server.receive_event()).await.unwrap()
        else {
            panic!("expected SUBSCRIBE from the client");
        };

        // Act
        let nothing_before_subscribe_ok = receives_no_event(&server).await;
        subscribe.ok(0, largest(3, 1)).await.unwrap();
        let event = within(server.receive_event()).await.unwrap();

        // Assert
        assert!(nothing_before_subscribe_ok);
        let SessionEvent::Fetch(fetch) = event else {
            panic!("expected the joining FETCH, got {event:?}");
        };
        assert_eq!(
            fetch.fetch.fetch_params,
            FetchParams::RelativeJoining {
                joining_request_id: subscribe.request_id(),
                joining_start: 0,
            }
        );
    }

    #[tokio::test]
    async fn request_new_group_is_not_sent_for_a_track_without_dynamic_groups() {
        // Arrange
        let track = subscribed_track("subscriber-new-group-static", Version::Draft18).await;

        // Act
        let sent = track
            .client
            .subscriber()
            .request_new_group(&track.subscription, Some(3))
            .await
            .unwrap();

        // Assert
        assert!(!sent);
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
        let (client, server) = spawn_connected_native_sessions("subscriber-current-group").await;
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
        let (client, server) = spawn_connected_native_sessions("subscriber-current-refused").await;
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
    async fn subscribe_current_group_sends_the_joining_fetch_before_subscribe_ok() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions("subscriber-current-pipelined").await;
        let mut subscriber = client.subscriber();
        let _request = tokio::spawn(async move {
            subscriber
                .subscribe_current_group("ns".to_string(), TRACK.to_string())
                .await
        });
        let SessionEvent::Subscribe(subscribe) = within(server.receive_event()).await.unwrap()
        else {
            panic!("expected SUBSCRIBE from the client");
        };

        // Act: the SUBSCRIBE is left unanswered
        let event = within(server.receive_event()).await.unwrap();

        // Assert
        let SessionEvent::Fetch(fetch) = event else {
            panic!("expected the joining FETCH, got {event:?}");
        };
        assert_eq!(
            fetch.fetch.fetch_params,
            FetchParams::RelativeJoining {
                joining_request_id: subscribe.request_id(),
                joining_start: 0,
            }
        );
    }

    #[tokio::test]
    async fn subscribe_current_group_yields_live_objects_for_a_track_without_content() {
        // Arrange
        let (client, server) = spawn_connected_native_sessions("subscriber-current-empty").await;
        let (mut reader, served) =
            start_subscribe_current_group(&client, &server, ContentExists::False).await;
        let SessionEvent::Fetch(fetch) = within(server.receive_event()).await.unwrap() else {
            panic!("expected the joining FETCH");
        };

        // Act: refused with INVALID_RANGE, as draft-14 §9.16.2 requires for a track without content
        fetch
            .error(RequestErrorCode::InvalidRange, String::new())
            .await
            .unwrap();
        send_subgroup_object(&server, &served, &fetch_object(location(0, 0), b"live")).await;
        let payloads = read_payloads(&mut reader, 1).await;

        // Assert
        assert_eq!(payloads, ["live"]);
    }

    #[tokio::test]
    async fn unsubscribe_namespace_reaches_the_peer_with_its_prefix() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions("subscriber-unsubscribe-namespace").await;
        subscribe_namespace_accepted(&client, &server, "room/a").await;

        // Act
        client
            .subscriber()
            .unsubscribe_namespace("room/a".to_string())
            .await
            .unwrap();
        let event = within(server.receive_event()).await.unwrap();

        // Assert
        let SessionEvent::UnsubscribeNamespace(handler) = event else {
            panic!("expected UNSUBSCRIBE_NAMESPACE, got {event:?}");
        };
        assert_eq!(handler.track_namespace_prefix(), "room/a");
    }

    #[tokio::test]
    async fn a_subscribe_does_not_wait_for_an_earlier_answer_when_following_go_away() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions_following_go_away("concurrent-carried-subscribe").await;
        let (first, _first_handler) = start_subscribe(&client, &server, "first").await;

        // Act
        let (second, second_handler) = start_subscribe(&client, &server, "second").await;

        // Assert
        assert_eq!(second_handler.track_name, "second");
        first.abort();
        second.abort();
    }

    #[tokio::test]
    async fn a_subscribe_namespace_does_not_wait_for_an_earlier_answer_when_following_go_away() {
        // Arrange
        let (client, server) = spawn_connected_native_sessions_following_go_away(
            "concurrent-carried-subscribe-namespace",
        )
        .await;
        let subscriber = client.subscriber();
        let first = tokio::spawn(async move {
            subscriber
                .subscribe_namespace("a".into(), NamespaceInterest::NamespacesAndTracks)
                .await
        });
        let SessionEvent::SubscribeNameSpace(_first_handler) =
            within(server.receive_event()).await.unwrap()
        else {
            panic!("expected SUBSCRIBE_NAMESPACE from the client");
        };

        // Act
        let (second, second_handler) = start_subscribe(&client, &server, "second").await;

        // Assert
        assert_eq!(second_handler.track_name, "second");
        first.abort();
        second.abort();
    }

    #[tokio::test]
    async fn an_unsubscribe_namespace_before_the_answer_is_not_carried_into_a_migration() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions_following_go_away("carried-unsubscribe-namespace")
                .await;
        let subscriber = client.subscriber();
        let session = subscriber.session.clone();
        let subscribed = tokio::spawn(async move {
            subscriber
                .subscribe_namespace("a".into(), NamespaceInterest::NamespacesAndTracks)
                .await
        });
        let SessionEvent::SubscribeNameSpace(handler) =
            within(server.receive_event()).await.unwrap()
        else {
            panic!("expected SUBSCRIBE_NAMESPACE from the client");
        };

        // Act
        within(client.subscriber().unsubscribe_namespace("a".into()))
            .await
            .unwrap();
        handler.ok().await.unwrap();
        within(subscribed).await.unwrap().unwrap();

        // Assert
        let carried = session
            .read_carried()
            .await
            .unwrap()
            .update(|carried| carried.namespaces.clone());
        assert!(carried.is_empty(), "{carried:?}");
    }

    #[tokio::test]
    async fn a_rejected_subscribe_namespace_is_not_carried_into_a_migration() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions_following_go_away("carried-rejected-namespace").await;
        let subscriber = client.subscriber();
        let session = subscriber.session.clone();
        let subscribed = tokio::spawn(async move {
            subscriber
                .subscribe_namespace("a".into(), NamespaceInterest::NamespacesAndTracks)
                .await
        });
        let SessionEvent::SubscribeNameSpace(handler) =
            within(server.receive_event()).await.unwrap()
        else {
            panic!("expected SUBSCRIBE_NAMESPACE from the client");
        };

        // Act
        handler
            .error(RequestErrorCode::InternalError, "rejected".to_string())
            .await
            .unwrap();
        let result = within(subscribed).await.unwrap();

        // Assert
        assert!(
            matches!(result, Err(RequestFailure::Rejected { .. })),
            "{result:?}"
        );
        let carried = session
            .read_carried()
            .await
            .unwrap()
            .update(|carried| carried.namespaces.clone());
        assert!(carried.is_empty(), "{carried:?}");
    }

    #[tokio::test]
    async fn a_rejected_subscribe_namespace_leaves_a_later_one_of_the_prefix_carried() {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions_following_go_away("carried-resubscribed-namespace")
                .await;
        let session = client.subscriber().session.clone();
        let subscribe = |subscriber: super::Subscriber| {
            tokio::spawn(async move {
                subscriber
                    .subscribe_namespace("a".into(), NamespaceInterest::NamespacesAndTracks)
                    .await
            })
        };
        let rejected = subscribe(client.subscriber());
        let SessionEvent::SubscribeNameSpace(first) = within(server.receive_event()).await.unwrap()
        else {
            panic!("expected SUBSCRIBE_NAMESPACE from the client");
        };
        within(client.subscriber().unsubscribe_namespace("a".into()))
            .await
            .unwrap();
        let SessionEvent::UnsubscribeNamespace(_) = within(server.receive_event()).await.unwrap()
        else {
            panic!("expected UNSUBSCRIBE_NAMESPACE from the client");
        };
        let _accepted = subscribe(client.subscriber());
        let SessionEvent::SubscribeNameSpace(_second) =
            within(server.receive_event()).await.unwrap()
        else {
            panic!("expected the second SUBSCRIBE_NAMESPACE from the client");
        };

        // Act
        first
            .error(RequestErrorCode::InternalError, "rejected".to_string())
            .await
            .unwrap();
        within(rejected).await.unwrap().unwrap_err();

        // Assert
        let carried = session
            .read_carried()
            .await
            .unwrap()
            .update(|carried| carried.namespaces.values().cloned().collect::<Vec<_>>());
        assert_eq!(
            carried,
            [("a".to_string(), NamespaceInterest::NamespacesAndTracks)]
        );
    }

    #[tokio::test]
    async fn an_unanswered_subscribe_is_logged_as_sent() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) = spawn_connected_native_sessions("subscriber-sent-log").await;

        // Act
        let (request, handler) = start_subscribe(&client, &server, "sent").await;

        // Assert
        let logged = telemetry.events("SUBSCRIBE sent");
        assert_eq!(logged.len(), 1);
        assert_eq!(
            logged[0].field("request_id"),
            Some(handler.request_id().to_string().as_str())
        );
        assert_eq!(logged[0].field("track_namespace"), Some("ns"));
        assert_eq!(logged[0].field("track_name"), Some("sent"));
        request.abort();
    }

    #[tokio::test]
    async fn subscribe_span_records_request_id_and_ok_outcome() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) = spawn_connected_native_sessions("subscriber-span-ok").await;
        let (request, handler) = start_subscribe(&client, &server, "span").await;

        // Act
        handler.ok(0, ContentExists::False).await.unwrap();
        within(request).await.unwrap().unwrap();

        // Assert
        let logged = telemetry.events("SUBSCRIBE_OK received");
        let spans = telemetry.closed_spans("moqt.subscriber.subscribe");
        assert_eq!(logged.len(), 1);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].field("request_id"), logged[0].field("request_id"));
        assert!(spans[0].field("request_id").is_some());
        assert_eq!(spans[0].field("outcome"), Some("ok"));
    }

    #[tokio::test]
    async fn subscribe_error_marks_the_span_rejected() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) = spawn_connected_native_sessions("subscriber-span-rejected").await;
        let (request, handler) = start_subscribe(&client, &server, "span").await;

        // Act
        handler
            .error(RequestErrorCode::InternalError, "rejected".to_string())
            .await
            .unwrap();
        let result = within(request).await.unwrap();

        // Assert
        assert!(matches!(result, Err(RequestFailure::Rejected { .. })));
        let spans = telemetry.closed_spans("moqt.subscriber.subscribe");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].field("outcome"), Some("rejected"));
    }

    #[tokio::test]
    async fn unanswered_subscribe_logs_timed_out_and_marks_the_span_timeout_on_draft14() {
        unanswered_subscribe_logs_timed_out_and_marks_the_span_timeout(Version::Draft14).await;
    }

    #[tokio::test]
    async fn unanswered_subscribe_logs_timed_out_and_marks_the_span_timeout_on_draft18() {
        unanswered_subscribe_logs_timed_out_and_marks_the_span_timeout(Version::Draft18).await;
    }

    /// One test per draft: the captured telemetry is per thread, and the
    /// sessions of an earlier draft would outlive its capture.
    async fn unanswered_subscribe_logs_timed_out_and_marks_the_span_timeout(version: Version) {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) = connected_sessions("subscriber-timeout", version).await;
        let (request, _handler) = start_subscribe(&client, &server, "unanswered").await;

        // Act
        tokio::time::pause();
        let result = request.await.unwrap();
        tokio::time::resume();

        // Assert
        assert!(matches!(result, Err(RequestFailure::Timeout)), "{result:?}");
        let sent = telemetry.events("SUBSCRIBE sent");
        let logged = telemetry.events("SUBSCRIBE timed out");
        let spans = telemetry.closed_spans("moqt.subscriber.subscribe");
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].field("request_id"), sent[0].field("request_id"));
        assert_eq!(logged[0].field("track_namespace"), Some("ns"));
        assert_eq!(logged[0].field("track_name"), Some("unanswered"));
        let duration_ms = logged[0]
            .field("duration_ms")
            .expect("duration_ms is logged");
        assert!(duration_ms.parse::<u64>().is_ok(), "{duration_ms}");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].field("request_id"), sent[0].field("request_id"));
        assert_eq!(spans[0].field("outcome"), Some("timeout"));
    }

    #[tokio::test]
    async fn subscribe_ok_received_log_carries_duration_ms() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) = spawn_connected_native_sessions("subscriber-duration").await;
        let (request, handler) = start_subscribe(&client, &server, "span").await;

        // Act
        handler.ok(0, ContentExists::False).await.unwrap();
        within(request).await.unwrap().unwrap();

        // Assert
        let logged = telemetry.events("SUBSCRIBE_OK received");
        assert_eq!(logged.len(), 1);
        let duration_ms = logged[0]
            .field("duration_ms")
            .expect("duration_ms is logged");
        assert!(duration_ms.parse::<u64>().unwrap() < HANDSHAKE_TIMEOUT.as_millis() as u64);
    }
}
