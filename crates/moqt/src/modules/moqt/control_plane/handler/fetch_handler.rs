use crate::modules::moqt::version::OutgoingResponse;
use std::{sync::Arc, time::Duration};

use thiserror::Error;

use crate::{
    TrackProperties,
    modules::{
        moqt::{
            control_plane::{
                control_messages::messages::{
                    fetch::Fetch,
                    fetch_ok::FetchOk,
                    parameters::{filter_type::FilterType, location::Location},
                },
                error_codes::{RequestErrorCode, RequestKind},
                handler::response_guard::ResponseGuard,
            },
            domains::{connection::Connection, session_context::SessionContext},
            version::Version,
        },
        transport::transport_send_stream::TransportSendError,
    },
    object::GroupOrder,
};

/// The state of the subscription a Joining FETCH names, as the publisher
/// holds it when it evaluates the FETCH.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JoinedSubscription {
    pub filter_type: FilterType,
    pub forward: bool,
}

/// Why the session's draft does not let a Joining FETCH join its subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum JoiningFetchRefusal {
    /// draft-14 §9.16.2: the publisher closes the session with
    /// PROTOCOL_VIOLATION.
    #[error("the joined subscription does not use the Largest Object filter")]
    NotLargestObjectFilter,
    /// draft-18 §10.12.2: the publisher answers REQUEST_ERROR INVALID_RANGE.
    #[error("the joined subscription has Forward State 0")]
    NotForwarding,
}

#[derive(Debug, Clone)]
pub struct FetchHandler {
    pub(crate) session_context: Arc<SessionContext>,
    pub request_id: u64,
    pub group_order: GroupOrder,
    pub fetch: Fetch,
    /// Sent in the FETCH_OK.
    pub track_properties: TrackProperties,
    guard: ResponseGuard,
}

impl FetchHandler {
    pub(crate) fn new(
        session_context: Arc<SessionContext>,
        connection: Arc<Connection>,
        fetch: Fetch,
    ) -> Self {
        let group_order = fetch.group_order;
        let guard = ResponseGuard::new(connection, fetch.request_id, RequestKind::Fetch);
        let request_id = guard.connection().logical_request_id(fetch.request_id);
        Self {
            session_context,
            request_id,
            group_order,
            fetch,
            track_properties: TrackProperties::default(),
            guard,
        }
    }

    /// The longest a relay should wait for objects it must fetch upstream
    /// before it reports them as unknown, `ZERO` meaning not at all
    /// (draft-18 §10.2.5 FILL_TIMEOUT). `None` when the FETCH sets none,
    /// which is always the case on draft-14.
    pub fn fill_timeout(&self) -> Option<Duration> {
        self.fetch.fill_timeout.map(Duration::from_millis)
    }

    /// Whether the response can mark a range of objects unknown and go on
    /// (draft-18 §11.4.4.2 End of Unknown Range). Where it cannot (draft-14),
    /// `FetchDataSender::report_unknown_range` resets the response instead.
    pub fn marks_unknown_ranges(&self) -> bool {
        match self.guard.connection().control.version() {
            Version::Draft14 => false,
            Version::Draft18 => true,
        }
    }

    /// Whether this Joining FETCH may join `subscription` under the session's
    /// draft (draft-14 §9.16.2, draft-18 §10.12.2); each refusal names the
    /// answer its draft requires. Not meaningful for a Standalone FETCH.
    pub fn check_joining(
        &self,
        subscription: JoinedSubscription,
    ) -> Result<(), JoiningFetchRefusal> {
        match self.guard.connection().control.version() {
            Version::Draft14 if subscription.filter_type != FilterType::LargestObject => {
                Err(JoiningFetchRefusal::NotLargestObjectFilter)
            }
            Version::Draft18 if !subscription.forward => Err(JoiningFetchRefusal::NotForwarding),
            Version::Draft14 | Version::Draft18 => Ok(()),
        }
    }

    /// Fails when FETCH_OK cannot be sent, or when `track_properties` hold a
    /// Mandatory Track Property the peer's draft does not understand: the
    /// FETCH is then refused with UNSUPPORTED_EXTENSION, NOT_SUPPORTED on
    /// draft-14 (draft-18 §2.5.1).
    pub async fn ok(&self, end_of_track: bool, end_location: Location) -> anyhow::Result<()> {
        self.guard
            .refuse_unless_understood(&self.track_properties)
            .await?;
        self.guard.mark_responded();
        let fetch_ok = FetchOk {
            request_id: self.guard.request_id(),
            group_order: self.group_order.delivered(),
            end_of_track,
            end_location,
            track_properties: self.track_properties.clone(),
        };
        self.guard
            .connection()
            .control
            .respond(OutgoingResponse::FetchOk(fetch_ok))
            .await?;
        Ok(())
    }

    pub async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.guard.reject(code, reason_phrase).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{FetchHandler, JoinedSubscription, JoiningFetchRefusal};
    use crate::{
        RequestErrorCode, RequestFailure, Session, SessionEvent,
        fetch::FetchOption,
        modules::moqt::version::Version,
        modules::test_support::{
            connect_sessions, mandatory_track_properties, spawn_connected_draft18_sessions,
            spawn_connected_dual_sessions, spawn_dual_server,
        },
        object::{GroupOrder, Location},
        subscribe::FilterType,
    };

    const START: Location = Location {
        group_id: 0,
        object_id: 0,
    };
    const END: Location = Location {
        group_id: 1,
        object_id: 0,
    };

    #[tokio::test]
    async fn fetch_ok_states_ascending_when_the_subscriber_leaves_the_order_to_the_publisher() {
        // Arrange
        let (port, accept) = spawn_dual_server("fetch-handler-group-order");
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .fetch(
                    "ns".to_string(),
                    "track".to_string(),
                    START,
                    END,
                    FetchOption {
                        group_order: GroupOrder::Publisher,
                        ..FetchOption::default()
                    },
                )
                .await
        });
        let SessionEvent::Fetch(handler) = server.receive_event().await.unwrap() else {
            panic!("expected FETCH from the client");
        };

        // Act
        handler.ok(false, END).await.unwrap();

        // Assert
        let fetch_handle = request.await.unwrap().unwrap();
        assert_eq!(fetch_handle.group_order, GroupOrder::Ascending);
    }

    async fn received_fetch(client: &Session, server: &Session) -> FetchHandler {
        received_fetch_with(client, server, FetchOption::default()).await
    }

    async fn received_fetch_with(
        client: &Session,
        server: &Session,
        option: FetchOption,
    ) -> FetchHandler {
        let mut subscriber = client.subscriber();
        tokio::spawn(async move {
            let _ = subscriber
                .fetch("ns".to_string(), "track".to_string(), START, END, option)
                .await;
        });
        let SessionEvent::Fetch(handler) = server.receive_event().await.unwrap() else {
            panic!("expected FETCH from the client");
        };
        handler
    }

    #[tokio::test]
    async fn a_draft14_fetch_ok_with_a_mandatory_track_property_is_sent_as_a_refusal() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("fetch-handler-mandatory").await;
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
        let SessionEvent::Fetch(mut handler) = server.receive_event().await.unwrap() else {
            panic!("expected FETCH from the client");
        };
        handler.track_properties = mandatory_track_properties();

        // Act
        let answered = handler.ok(false, END).await;

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
    async fn a_draft14_fetch_has_no_fill_timeout() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("fetch-handler-fill-14").await;

        // Act
        let handler = received_fetch(&client, &server).await;

        // Assert: draft-14 §9.16 defines no FILL_TIMEOUT
        assert_eq!(handler.fill_timeout(), None);
    }

    #[tokio::test]
    async fn a_fetch_option_fill_timeout_is_sent_on_draft18_only() {
        let fill_timeout = Duration::from_millis(1500);
        for (version, (client, server), expected) in [
            (
                Version::Draft14,
                spawn_connected_dual_sessions("fetch-option-fill-14").await,
                None,
            ),
            (
                Version::Draft18,
                spawn_connected_draft18_sessions("fetch-option-fill-18", "moqt").await,
                Some(fill_timeout),
            ),
        ] {
            // Act
            let handler = received_fetch_with(
                &client,
                &server,
                FetchOption {
                    fill_timeout: Some(fill_timeout),
                    ..FetchOption::default()
                },
            )
            .await;

            // Assert: draft-18 §10.2.5 FILL_TIMEOUT; draft-14 §9.16 has none
            assert_eq!(handler.fill_timeout(), expected, "{version:?}");
        }
    }

    #[tokio::test]
    async fn only_a_draft18_fetch_response_can_mark_objects_unknown() {
        for (version, (client, server), expected) in [
            (
                Version::Draft14,
                spawn_connected_dual_sessions("fetch-marks-unknown-14").await,
                false,
            ),
            (
                Version::Draft18,
                spawn_connected_draft18_sessions("fetch-marks-unknown-18", "moqt").await,
                true,
            ),
        ] {
            // Act
            let handler = received_fetch(&client, &server).await;

            // Assert: draft-18 §11.4.4.2 End of Unknown Range; draft-14 §10.4.4 has no such marker
            assert_eq!(handler.marks_unknown_ranges(), expected, "{version:?}");
        }
    }

    #[tokio::test]
    async fn a_fetch_refused_with_malformed_track_fails_as_malformed_track() {
        for (version, (client, server)) in [
            (
                Version::Draft14,
                spawn_connected_dual_sessions("fetch-malformed-track-14").await,
            ),
            (
                Version::Draft18,
                spawn_connected_draft18_sessions("fetch-malformed-track-18", "moqt").await,
            ),
        ] {
            // Arrange
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

            // Act: the publisher, e.g. a relay that latched the track, refuses it
            handler
                .error(RequestErrorCode::MalformedTrack, "malformed".to_string())
                .await
                .unwrap();

            // Assert: draft-14 §9.18 FETCH_ERROR 0x9, draft-18 §10.6 0x12
            let result = request.await.unwrap();
            assert!(
                matches!(
                    &result,
                    Err(RequestFailure::Rejected {
                        code: RequestErrorCode::MalformedTrack,
                        ..
                    })
                ),
                "{version:?}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_draft14_joining_fetch_joins_only_a_largest_object_subscription() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("fetch-handler-join-14").await;
        let handler = received_fetch(&client, &server).await;

        // Act
        let next_group_start = handler.check_joining(JoinedSubscription {
            filter_type: FilterType::NextGroupStart,
            forward: true,
        });
        let not_forwarding = handler.check_joining(JoinedSubscription {
            filter_type: FilterType::LargestObject,
            forward: false,
        });

        // Assert: draft-14 §9.16.2 sets no Forward State condition
        assert_eq!(
            next_group_start,
            Err(JoiningFetchRefusal::NotLargestObjectFilter)
        );
        assert_eq!(not_forwarding, Ok(()));
    }

    #[tokio::test]
    async fn a_draft18_joining_fetch_joins_only_a_forwarding_subscription() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("fetch-handler-join-18", "moqt").await;
        let handler = received_fetch(&client, &server).await;

        // Act
        let not_forwarding = handler.check_joining(JoinedSubscription {
            filter_type: FilterType::LargestObject,
            forward: false,
        });
        let next_group_start = handler.check_joining(JoinedSubscription {
            filter_type: FilterType::NextGroupStart,
            forward: true,
        });

        // Assert: draft-18 §10.12.2 sets no filter condition
        assert_eq!(not_forwarding, Err(JoiningFetchRefusal::NotForwarding));
        assert_eq!(next_group_start, Ok(()));
    }
}
