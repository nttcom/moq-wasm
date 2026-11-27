use crate::modules::moqt::control_plane::control_messages::messages::parameters::trace_context::TraceContext;
use crate::modules::moqt::version::{OutgoingResponse, RequestSummary};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    SubscriberInitiatedSubscription, Subscription, TrackProperties,
    modules::moqt::{
        control_plane::{
            control_messages::messages::{
                parameters::content_exists::ContentExists, subscribe::Subscribe,
                subscribe_ok::SubscribeOk,
            },
            error_codes::{RequestErrorCode, RequestKind},
            handler::response_guard::ResponseGuard,
        },
        domains::{
            connection::{Connection, logical_id, wire_part},
            session_context::SessionContext,
        },
    },
    modules::transport::transport_send_stream::TransportSendError,
    object::GroupOrder,
    subscribe::FilterType,
};

#[derive(Debug, Clone)]
pub struct SubscribeHandler {
    pub(crate) session_context: Arc<SessionContext>,
    request_id: u64,
    pub track_namespace: String,
    pub track_namespace_tuple: Vec<String>,
    pub track_name: String,
    pub subscriber_priority: u8,
    pub group_order: GroupOrder,
    pub forward: bool,
    pub filter_type: FilterType,
    /// The SUBSCRIBE's NEW_GROUP_REQUEST (draft-18 §10.2.13): the largest
    /// Group ID the subscriber knows plus 1, 0 when it knows none.
    pub new_group_request: Option<u64>,
    /// The SUBSCRIBE's DELIVERY TIMEOUT in milliseconds (draft-14 §9.2.1.2,
    /// draft-18 §10.2.4 OBJECT_DELIVERY_TIMEOUT).
    pub delivery_timeout: Option<u64>,
    /// Sent in the SUBSCRIBE_OK.
    pub track_properties: TrackProperties,
    trace_context: Option<TraceContext>,
    guard: ResponseGuard,
    /// Set on a session that follows GOAWAY.
    served: Option<Arc<ServedSubscribe>>,
}

/// Counts a SUBSCRIBE as served on the connection it arrived on from its
/// receipt, ordered before its UNSUBSCRIBE, until the application refuses
/// it or drops it unanswered; a migration does not close that connection
/// while it is served.
#[derive(Debug)]
struct ServedSubscribe {
    session_context: Arc<SessionContext>,
    connection: Arc<Connection>,
    request_id: u64,
    accepted: AtomicBool,
}

impl ServedSubscribe {
    fn refused(&self) {
        if !self.accepted.load(Ordering::SeqCst) {
            self.connection.stop_serving(self.request_id);
            self.session_context.note_served_changed();
        }
    }
}

impl Drop for ServedSubscribe {
    fn drop(&mut self) {
        self.refused();
    }
}

impl SubscribeHandler {
    /// The TRACE_CONTEXT the request carried, if the peer sent a valid one.
    pub fn trace_context(&self) -> Option<&TraceContext> {
        self.trace_context.as_ref()
    }

    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) fn new(
        session_context: Arc<SessionContext>,
        connection: Arc<Connection>,
        subscribe_message: Subscribe,
    ) -> Self {
        let guard = ResponseGuard::new(
            connection,
            RequestSummary::new(
                RequestKind::Subscribe,
                subscribe_message.request_id,
                Some(&subscribe_message.track_namespace),
                Some(&subscribe_message.track_name),
            ),
        );
        let request_id = guard
            .connection()
            .logical_request_id(subscribe_message.request_id);
        let served = session_context.follows_go_away().then(|| {
            guard.connection().serve(request_id);
            Arc::new(ServedSubscribe {
                session_context: session_context.clone(),
                connection: guard.connection().clone(),
                request_id,
                accepted: AtomicBool::new(false),
            })
        });
        Self {
            served,
            session_context,
            guard,
            request_id,
            track_namespace: subscribe_message.track_namespace.join("/"),
            track_namespace_tuple: subscribe_message.track_namespace,
            track_name: subscribe_message.track_name,
            subscriber_priority: subscribe_message.subscriber_priority,
            group_order: subscribe_message.group_order,
            forward: subscribe_message.forward,
            filter_type: subscribe_message.filter_type,
            new_group_request: subscribe_message.new_group_request,
            delivery_timeout: subscribe_message.delivery_timeout,
            track_properties: TrackProperties::default(),
            trace_context: subscribe_message.trace_context,
        }
    }

    /// Returns the track alias to pass to `into_subscription`. Fails like
    /// `ok_with_track_alias`.
    pub async fn ok(&self, expires: u64, content_exists: ContentExists) -> anyhow::Result<u64> {
        let track_alias = self.allocate_track_alias();
        self.ok_with_track_alias(track_alias, expires, content_exists)
            .await?;
        Ok(track_alias)
    }

    /// `track_alias` comes from this handler's `allocate_track_alias`. Fails
    /// when SUBSCRIBE_OK cannot be sent, or when `track_properties` hold a
    /// Mandatory Track Property the peer's draft does not understand: the
    /// SUBSCRIBE is then refused with UNSUPPORTED_EXTENSION, NOT_SUPPORTED
    /// on draft-14 (draft-18 §2.5.1).
    pub async fn ok_with_track_alias(
        &self,
        track_alias: u64,
        expires: u64,
        content_exists: ContentExists,
    ) -> anyhow::Result<()> {
        let answered = self.send_ok(track_alias, expires, content_exists).await;
        if let Some(served) = &self.served {
            match answered {
                Ok(()) => served.accepted.store(true, Ordering::SeqCst),
                Err(_) => served.refused(),
            }
        }
        answered
    }

    async fn send_ok(
        &self,
        track_alias: u64,
        expires: u64,
        content_exists: ContentExists,
    ) -> anyhow::Result<()> {
        self.guard
            .refuse_unless_understood(&self.track_properties)
            .await?;
        let subscribe_ok = SubscribeOk {
            request_id: self.guard.request_id(),
            track_alias: wire_part(track_alias),
            expires,
            group_order: delivered_group_order(self.group_order, &self.track_properties),
            content_exists,
            track_properties: self.track_properties.clone(),
        };
        self.guard
            .accept(OutgoingResponse::SubscribeOk(subscribe_ok))
            .await?;
        Ok(())
    }

    /// Whether the subscriber's draft can be sent a track with
    /// `track_properties`; `ok*` refuses the SUBSCRIBE when it cannot
    /// (draft-18 §2.5.1).
    pub fn understands(&self, track_properties: &TrackProperties) -> bool {
        self.guard
            .connection()
            .control
            .version()
            .understands(track_properties)
    }

    /// The track alias to pass to `ok_with_track_alias` and `into_subscription`.
    /// No track served on another connection of the session has it.
    pub fn allocate_track_alias(&self) -> u64 {
        let connection = self.guard.connection();
        logical_id(connection.generation, connection.allocate_track_alias())
    }

    pub async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        let rejected = self.guard.reject(code, reason_phrase).await;
        if let Some(served) = &self.served {
            served.refused();
        }
        rejected
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    /// Builds the subscription this handler represents. `track_alias` is the
    /// alias `ok` returned or the one passed to `ok_with_track_alias`.
    /// Receiving a SUBSCRIBE makes it subscriber-initiated.
    pub fn into_subscription(&self, track_alias: u64) -> Subscription {
        Subscription::SubscriberInitiated(SubscriberInitiatedSubscription::from_subscribe_handler(
            track_alias,
            self,
        ))
    }
}

/// The order a subscription is delivered in, which a draft-14 SUBSCRIBE_OK
/// or TRACK_STATUS_OK states (§9.8, §9.21): the one the subscriber asked for,
/// else the track's DEFAULT PUBLISHER GROUP ORDER, else Ascending.
pub(crate) fn delivered_group_order(
    requested: GroupOrder,
    track_properties: &TrackProperties,
) -> GroupOrder {
    match requested {
        GroupOrder::Publisher => track_properties
            .group_order()
            .unwrap_or(GroupOrder::Ascending),
        requested => requested,
    }
}

#[cfg(test)]
mod tests {
    use super::SubscribeHandler;
    use crate::subscribe::SubscribeOption;
    use crate::{
        RequestErrorCode, RequestFailure, Session, SessionEvent, Version,
        modules::{
            moqt::{
                control_plane::control_messages::messages::unsubscribe::Unsubscribe,
                version::Notification,
            },
            test_support::{
                DRAFTS, connected_sessions, connected_sessions_following_go_away,
                mandatory_track_properties, next_event, spawn_connected_native_sessions,
            },
        },
        object::{ContentExists, GroupOrder},
    };

    #[tokio::test]
    async fn a_draft14_subscribe_ok_with_a_mandatory_track_property_is_sent_as_a_refusal() {
        // Arrange
        let (client, server) = spawn_connected_native_sessions("subscribe-handler-mandatory").await;
        let request = tokio::spawn(async move {
            client
                .subscriber()
                .subscribe(
                    "ns".to_string(),
                    "track".to_string(),
                    SubscribeOption::default(),
                )
                .await
        });
        let SessionEvent::Subscribe(mut handler) = next_event(&server).await else {
            panic!("expected SUBSCRIBE from the client");
        };
        handler.track_properties = mandatory_track_properties();

        // Act
        let answered = handler.ok(0, ContentExists::False).await;

        // Assert: draft-18 §2.5.1; draft-14 refuses with NOT_SUPPORTED
        assert!(answered.is_err());
        assert!(matches!(
            request.await.unwrap(),
            Err(RequestFailure::Rejected {
                code: RequestErrorCode::NotSupported,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_draft14_subscribe_ok_with_a_value_of_2_62_is_sent_as_an_internal_error() {
        // Arrange
        let (client, server) = spawn_connected_native_sessions("subscribe-handler-2-62").await;
        let request = tokio::spawn(async move {
            client
                .subscriber()
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

        // Act
        let answered = handler.ok(1 << 62, ContentExists::False).await;

        // Assert
        assert!(answered.is_err());
        assert!(matches!(
            request.await.unwrap(),
            Err(RequestFailure::Rejected {
                code: RequestErrorCode::InternalError,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_draft14_refusal_with_unsupported_extension_reaches_the_subscriber_as_not_supported()
    {
        // Arrange
        let (client, server) =
            spawn_connected_native_sessions("subscribe-handler-unsupported-extension").await;
        let request = tokio::spawn(async move {
            client
                .subscriber()
                .subscribe(
                    "ns".to_string(),
                    "track".to_string(),
                    SubscribeOption::default(),
                )
                .await
        });
        let SessionEvent::Subscribe(handler) = next_event(&server).await else {
            panic!("expected SUBSCRIBE from the client");
        };

        // Act
        handler
            .error(RequestErrorCode::UnsupportedExtension, String::new())
            .await
            .unwrap();

        // Assert: draft-18 §2.5.1 relays UNSUPPORTED_EXTENSION, which draft-14 SUBSCRIBE_ERROR (§9.9) lacks; NOT_SUPPORTED stands in
        assert!(matches!(
            request.await.unwrap(),
            Err(RequestFailure::Rejected {
                code: RequestErrorCode::NotSupported,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn subscribe_ok_states_ascending_when_the_subscriber_leaves_the_order_to_the_publisher() {
        for version in DRAFTS {
            // Arrange
            let (client, server) =
                connected_sessions("subscribe-handler-group-order", version).await;
            let request = tokio::spawn(async move {
                client
                    .subscriber()
                    .subscribe(
                        "ns".to_string(),
                        "track".to_string(),
                        SubscribeOption {
                            group_order: GroupOrder::Publisher,
                            ..SubscribeOption::default()
                        },
                    )
                    .await
            });
            let SessionEvent::Subscribe(handler) = next_event(&server).await else {
                panic!("expected SUBSCRIBE from the client");
            };

            // Act
            handler.ok(0, ContentExists::False).await.unwrap();

            // Assert
            let subscription = request.await.unwrap().unwrap();
            assert_eq!(subscription.group_order(), GroupOrder::Ascending);
        }
    }

    #[tokio::test]
    async fn subscribe_ok_hands_its_max_cache_duration_to_the_subscriber() {
        for version in DRAFTS {
            // Arrange
            let (client, server) =
                connected_sessions("subscribe-handler-max-cache-duration", version).await;
            let request = tokio::spawn(async move {
                client
                    .subscriber()
                    .subscribe(
                        "ns".to_string(),
                        "track".to_string(),
                        SubscribeOption::default(),
                    )
                    .await
            });
            let SessionEvent::Subscribe(mut handler) = next_event(&server).await else {
                panic!("expected SUBSCRIBE from the client");
            };
            handler.track_properties.set_max_cache_duration(Some(2_000));

            // Act
            handler.ok(0, ContentExists::False).await.unwrap();

            // Assert
            let subscription = request.await.unwrap().unwrap();
            assert_eq!(
                subscription.track_properties().max_cache_duration(),
                Some(2_000)
            );
        }
    }

    #[tokio::test]
    async fn exposes_track_namespace_as_tuple_and_joined_string() {
        for version in DRAFTS {
            // Arrange
            let (client, server) = connected_sessions("subscribe-handler-namespace", version).await;
            let request = tokio::spawn(async move {
                client
                    .subscriber()
                    .subscribe(
                        "a/b/c".to_string(),
                        "track".to_string(),
                        SubscribeOption::default(),
                    )
                    .await
            });

            // Act
            let SessionEvent::Subscribe(handler) = next_event(&server).await else {
                panic!("expected SUBSCRIBE from the client");
            };

            // Assert
            assert_eq!(handler.track_namespace_tuple, vec!["a", "b", "c"]);
            assert_eq!(handler.track_namespace, "a/b/c");
            request.abort();
        }
    }

    /// The client of a session following GOAWAY received the server's
    /// SUBSCRIBE; returns (client, server, the client's handler).
    async fn subscribe_received_following_go_away(
        name: &str,
    ) -> (Session, Session, SubscribeHandler) {
        let (client, server) = connected_sessions_following_go_away(name, Version::Draft14).await;
        let mut subscriber = server.subscriber();
        tokio::spawn(async move {
            subscriber
                .subscribe(
                    "ns".to_string(),
                    "track".to_string(),
                    SubscribeOption::default(),
                )
                .await
        });
        let SessionEvent::Subscribe(handler) = next_event(&client).await else {
            panic!("expected SUBSCRIBE from the server");
        };
        (client, server, handler)
    }

    fn serves(client: &Session) -> bool {
        client.context().serves_on(&client.context().connection())
    }

    #[tokio::test]
    async fn a_subscribe_unsubscribed_before_it_is_accepted_is_not_served() {
        // Arrange
        let (client, server, handler) =
            subscribe_received_following_go_away("subscribe-handler-unsubscribed-first").await;
        server
            .context()
            .connection()
            .control
            .notify(Notification::Unsubscribe(Unsubscribe {
                request_id: handler.request_id(),
            }))
            .await
            .unwrap();
        let SessionEvent::Unsubscribe(_) = next_event(&client).await else {
            panic!("expected UNSUBSCRIBE from the server");
        };

        // Act
        handler.ok(0, ContentExists::False).await.unwrap();

        // Assert: a migration need not wait for an UNSUBSCRIBE that already came
        assert!(!serves(&client));
    }

    #[tokio::test]
    async fn a_subscribe_is_served_from_its_receipt_until_it_is_refused() {
        // Arrange
        let (client, _server, handler) =
            subscribe_received_following_go_away("subscribe-handler-refused").await;
        let served_on_receipt = serves(&client);

        // Act
        handler
            .error(RequestErrorCode::InternalError, String::new())
            .await
            .unwrap();

        // Assert
        assert!(served_on_receipt);
        assert!(!serves(&client));
    }

    #[tokio::test]
    async fn a_subscribe_dropped_unanswered_is_no_longer_served() {
        // Arrange
        let (client, _server, handler) =
            subscribe_received_following_go_away("subscribe-handler-dropped").await;

        // Act
        drop(handler);

        // Assert
        assert!(!serves(&client));
    }
}
