use std::sync::Arc;

use crate::modules::moqt::control_plane::error_codes::{PublishDoneCode, RequestErrorCode};

use crate::{
    RequestFailure,
    modules::moqt::{
        control_plane::{
            control_messages::messages::{
                publish::Publish, publish_namespace::PublishNamespace,
                publish_namespace_done::PublishNamespaceDone, publish_ok::PublishOk,
            },
            enums::ResponseMessage,
            options::PublishOption,
        },
        data_plane::stream::{
            fetch_data_sender::FetchDataSender, stream_data_sender_factory::StreamDataSenderFactory,
        },
        domains::{
            connection::{Connection, logical_id},
            namespace_tuple,
            pending_publish_namespace::PendingPublishNamespace,
            send_targets::SendTargets,
            session_context::{Publication, PublicationTarget, SessionContext},
            subscription::{PublisherInitiatedSubscription, Subscription},
        },
        version::{Notification, OutgoingRequest},
    },
    object::DatagramSender,
};

pub struct Publisher {
    pub(crate) session: Arc<SessionContext>,
}

impl Publisher {
    pub async fn publish_namespace(&self, namespace: String) -> Result<(), RequestFailure> {
        self.begin_publish_namespace(namespace)
            .await?
            .accepted()
            .await
    }

    pub async fn begin_publish_namespace(
        &self,
        namespace: String,
    ) -> Result<PendingPublishNamespace, RequestFailure> {
        let mut carried = self.session.lock_carried().await;
        let pending = send_publish_namespace(&self.session.connection(), namespace.clone()).await?;
        if let Some(carried) = carried.as_mut() {
            carried.published_namespaces.push(namespace);
        }
        Ok(pending)
    }

    /// Withdraws a previous PUBLISH_NAMESPACE. Fire-and-forget: the spec
    /// defines no response message for PUBLISH_NAMESPACE_DONE.
    pub async fn publish_namespace_done(&self, namespace: String) -> anyhow::Result<()> {
        if let Some(carried) = self.session.lock_carried().await.as_mut() {
            carried
                .published_namespaces
                .retain(|carried| *carried != namespace);
        }
        let vec_namespace = namespace_tuple(&namespace);
        for connection in self.session.live_connections() {
            connection
                .control
                .notify(Notification::PublishNamespaceDone(
                    PublishNamespaceDone::new(vec_namespace.clone()),
                ))
                .await?;
        }
        Ok(())
    }

    /// Tells the peer this side no longer serves `namespace` (draft-14
    /// §9.27). Fire-and-forget: the draft defines no response.
    pub async fn publish_namespace_cancel(
        &self,
        namespace: String,
        code: RequestErrorCode,
        error_reason: String,
    ) -> anyhow::Result<()> {
        self.session
            .connection()
            .control
            .notify(Notification::PublishNamespaceCancel {
                namespace: namespace_tuple(&namespace),
                code,
                reason: error_reason,
            })
            .await?;
        Ok(())
    }

    /// Ends a subscription this endpoint publishes on (draft-14 §9.12), on
    /// every connection it is on.
    pub async fn publish_done(
        &self,
        subscription: &Subscription,
        status_code: PublishDoneCode,
        stream_count: u64,
        error_reason: String,
    ) -> anyhow::Result<()> {
        let request_id = subscription.request_id();
        let targets: Vec<(Arc<Connection>, u64)> = match self.session.remove_publication(request_id)
        {
            Some(publication) => publication
                .targets
                .into_iter()
                .filter_map(|target| Some((target.connection.upgrade()?, target.request_id)))
                .collect(),
            None => self.session.connection_of(request_id).into_iter().collect(),
        };
        for (connection, wire_request_id) in targets {
            connection.stop_serving(request_id);
            self.session.note_served_changed();
            connection
                .control
                .notify(Notification::PublishDone {
                    request_id: wire_request_id,
                    code: status_code,
                    stream_count,
                    reason: error_reason.clone(),
                })
                .await?;
        }
        Ok(())
    }

    pub async fn publish(
        &self,
        track_namespace: String,
        track_name: String,
        option: PublishOption,
    ) -> Result<Subscription, RequestFailure> {
        let _carried = self.session.lock_carried().await;
        let connection = self.session.connection();
        let (publish_ok, track_alias) =
            send_publish(&connection, &track_namespace, &track_name, &option).await?;
        let request_id = logical_id(connection.generation, publish_ok.request_id);
        if self.session.follows_go_away() {
            self.session.add_publication(
                request_id,
                Publication {
                    track_namespace: track_namespace.clone(),
                    track_name: track_name.clone(),
                    option: option.clone(),
                    targets: vec![PublicationTarget {
                        connection: Arc::downgrade(&connection),
                        track_alias,
                        request_id: publish_ok.request_id,
                    }],
                },
            );
        }
        Ok(Subscription::PublisherInitiated(
            PublisherInitiatedSubscription::new(
                track_namespace,
                track_name,
                logical_id(connection.generation, track_alias),
                PublishOk {
                    request_id,
                    ..publish_ok
                },
                option.max_cache_duration,
            )
            .with_content_exists(option.content_exists),
        ))
    }

    pub fn create_stream(&self, subscription: &Subscription) -> StreamDataSenderFactory {
        StreamDataSenderFactory::new(self.send_targets(subscription))
    }

    pub fn create_datagram(&self, subscription: &Subscription) -> DatagramSender {
        DatagramSender::new(self.send_targets(subscription))
    }

    /// Opens the response stream of the FETCH `request_id` on the connection
    /// it arrived on; fails once that connection is gone.
    pub async fn create_fetch_stream(&self, request_id: u64) -> anyhow::Result<FetchDataSender> {
        let (connection, wire_request_id) = self
            .session
            .connection_of(request_id)
            .ok_or_else(|| anyhow::anyhow!("the connection of FETCH {request_id} is gone"))?;
        let send_stream = connection.transport.open_uni().await?;
        FetchDataSender::new(&connection, send_stream, wire_request_id).await
    }

    fn send_targets(&self, subscription: &Subscription) -> SendTargets {
        let request_id = subscription.request_id();
        if self.session.has_publication(request_id) {
            return SendTargets::Publication {
                session: self.session.clone(),
                request_id,
            };
        }
        let (connection, track_alias) = self
            .session
            .connection_of(subscription.track_alias())
            .map(|(connection, track_alias)| (Arc::downgrade(&connection), track_alias))
            .unwrap_or_default();
        SendTargets::Connection {
            connection,
            track_alias,
        }
    }
}

pub(crate) async fn send_publish_namespace(
    connection: &Arc<Connection>,
    namespace: String,
) -> Result<PendingPublishNamespace, RequestFailure> {
    let vec_namespace = namespace_tuple(&namespace);
    let request_id = connection.control.next_request_id();
    let pending = connection
        .send_request(OutgoingRequest::PublishNamespace(PublishNamespace::new(
            request_id,
            vec_namespace,
            vec![],
        )))
        .await?;
    Ok(PendingPublishNamespace {
        request_id,
        pending,
    })
}

/// The PUBLISH_OK and the track alias this side chose, with `connection`'s
/// wire ids.
pub(crate) async fn send_publish(
    connection: &Arc<Connection>,
    track_namespace: &str,
    track_name: &str,
    option: &PublishOption,
) -> Result<(PublishOk, u64), RequestFailure> {
    let track_alias = connection.allocate_track_alias();
    let request_id = connection.control.next_request_id();
    let publish = Publish {
        request_id,
        track_namespace_tuple: namespace_tuple(track_namespace),
        track_name: track_name.to_string(),
        track_alias,
        group_order: option.group_order,
        content_exists: option.content_exists,
        forward: option.forward,
        authorization_tokens: vec![],
        delivery_timeout: None,
        max_duration: option.max_cache_duration,
    };
    // A late PUBLISH_OK is discarded: the peer keeps subscription state
    // but receives no objects.
    let response = connection
        .send_request(OutgoingRequest::Publish(publish))
        .await?
        .response()
        .await?;
    match response {
        ResponseMessage::PublishOk(message) if message.request_id == request_id => {
            tracing::info!("Publish ok");
            Ok((message, track_alias))
        }
        ResponseMessage::PublishOk(_) => Err(RequestFailure::ProtocolViolation(
            "response for another request ID",
        )),
        ResponseMessage::PublishError(request_id, error_code, reason_phrase) => {
            tracing::info!("Publish error");
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
    use tokio::task::JoinHandle;

    use crate::{
        RequestErrorCode, RequestFailure, Session, SessionEvent, Subscription,
        modules::{
            moqt::runtime::tasks::publish_done_expiry_task::PUBLISH_DONE_GRACE_PERIOD,
            test_support::{
                HANDSHAKE_TIMEOUT, first_subgroup_stream, object_with_extensions,
                read_subgroup_objects, send_subgroup_object, spawn_connected_dual_sessions,
                subgroup_receiver, within,
            },
        },
        object::{Location, Object},
        publish::{PublishDoneCode, PublishHandler, PublishOption},
        subscribe::FilterType,
    };

    const WAIT_TIMEOUT: Duration = Duration::from_secs(3);

    #[tokio::test]
    async fn publish_takes_its_track_alias_from_the_counter_subscribe_ok_uses() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("publisher-track-alias").await;
        let session = client.publisher().session;
        let request = tokio::spawn(async move {
            client
                .publisher()
                .publish(
                    "ns".to_string(),
                    "track".to_string(),
                    PublishOption::default(),
                )
                .await
        });
        let SessionEvent::Publish(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH from the client");
        };

        // Act
        let subscribe_ok_track_alias = session.connection().allocate_track_alias();

        // Assert
        assert_eq!(subscribe_ok_track_alias, handler.track_alias + 1);
        request.abort();
    }

    #[tokio::test]
    async fn pending_publish_namespace_is_accepted_by_publish_namespace_ok() {
        // Arrange
        let (client, server) =
            spawn_connected_dual_sessions("publisher-pending-publish-namespace").await;
        let pending = tokio::time::timeout(
            WAIT_TIMEOUT,
            client
                .publisher()
                .begin_publish_namespace("a/b".to_string()),
        )
        .await
        .expect("sending PUBLISH_NAMESPACE should not wait for the answer")
        .unwrap();
        let SessionEvent::PublishNamespace(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH_NAMESPACE from the client");
        };

        // Act
        handler.ok().await.unwrap();

        // Assert
        tokio::time::timeout(WAIT_TIMEOUT, pending.accepted())
            .await
            .expect("PUBLISH_NAMESPACE_OK should resolve the pending request")
            .unwrap();
    }

    #[tokio::test]
    async fn publish_namespace_cancel_reaches_the_peer_with_its_code_and_reason() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("publisher-namespace-cancel").await;

        // Act
        client
            .publisher()
            .publish_namespace_cancel(
                "live/room".to_string(),
                RequestErrorCode::Uninterested,
                "moved".to_string(),
            )
            .await
            .unwrap();

        // Assert
        let SessionEvent::PublishNamespaceCancel(cancel) = server.receive_event().await.unwrap()
        else {
            panic!("expected PUBLISH_NAMESPACE_CANCEL from the client");
        };
        assert_eq!(cancel.track_namespace(), "live/room");
        assert_eq!(cancel.error_code(), RequestErrorCode::Uninterested);
        assert_eq!(cancel.error_reason(), "moved");
    }

    async fn pending_publish(
        client: &Session,
        server: &Session,
        track_name: &str,
    ) -> (
        JoinHandle<Result<Subscription, RequestFailure>>,
        PublishHandler,
    ) {
        let publisher = client.publisher();
        let track_name = track_name.to_string();
        let request = tokio::spawn(async move {
            publisher
                .publish("ns".to_string(), track_name, PublishOption::default())
                .await
        });
        let SessionEvent::Publish(handler) = within(server.receive_event()).await.unwrap() else {
            panic!("expected PUBLISH from the client");
        };
        (request, handler)
    }

    async fn active_publish(
        client: &Session,
        server: &Session,
        track_name: &str,
    ) -> (Subscription, Subscription) {
        let (request, handler) = pending_publish(client, server, track_name).await;
        handler.accept_data_receiver().await;
        let accepted = handler.ok(128, FilterType::LargestObject).await.unwrap();
        (within(request).await.unwrap().unwrap(), accepted)
    }

    #[tokio::test]
    async fn cancelled_publish_sends_done_for_late_ok_and_allows_later_publication() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("publish-cancel-late-ok").await;
        let (request, handler) = pending_publish(&client, &server, "cancelled").await;
        let expected = object_with_extensions();

        // Act
        request.abort();
        let cancelled = request.await.unwrap_err();
        let (done, received) = within(async {
            handler.ok(128, FilterType::LargestObject).await.unwrap();
            let SessionEvent::PublishDone(done) = server.receive_event().await.unwrap() else {
                panic!("expected PUBLISH_DONE after the late PUBLISH_OK");
            };
            let (published, accepted) = active_publish(&client, &server, "later").await;
            send_subgroup_object(&client, &published, &expected).await;
            let mut receiver = first_subgroup_stream(&server, &accepted).await;
            let received = read_subgroup_objects(&mut receiver).await;
            (done, received)
        })
        .await;

        // Assert
        assert!(cancelled.is_cancelled());
        assert_eq!(done.request_id(), handler.request_id);
        assert_eq!(done.status_code(), PublishDoneCode::InternalError);
        assert_eq!(done.stream_count(), 0);
        assert_eq!(received, vec![expected]);
    }

    #[tokio::test]
    async fn publish_done_ends_only_its_publication() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("publish-done-isolation").await;
        let (ended, ended_peer) = within(active_publish(&client, &server, "ended")).await;
        let (kept, kept_peer) = within(active_publish(&client, &server, "kept")).await;
        let mut ended_reader = server.subscriber().track_reader(&ended_peer);
        let before = object_with_extensions();
        let after = Object {
            location: Location {
                group_id: before.location.group_id + 1,
                object_id: 0,
            },
            payload: Bytes::from_static(b"after the other publication ended"),
            ..object_with_extensions()
        };
        within(send_subgroup_object(&client, &kept, &before)).await;
        let mut streams = within(subgroup_receiver(&server, &kept_peer)).await;
        let mut first_receiver = within(streams.next()).await.unwrap();
        let received_before = within(read_subgroup_objects(&mut first_receiver)).await;

        // Act
        let done = within(async {
            client
                .publisher()
                .publish_done(
                    &ended,
                    PublishDoneCode::TrackEnded,
                    0,
                    "finished".to_string(),
                )
                .await
                .unwrap();
            let SessionEvent::PublishDone(done) = server.receive_event().await.unwrap() else {
                panic!("expected PUBLISH_DONE for the ended publication");
            };
            done
        })
        .await;
        tokio::time::pause();
        let ended_object = tokio::time::timeout(
            PUBLISH_DONE_GRACE_PERIOD + HANDSHAKE_TIMEOUT,
            ended_reader.next_object(),
        )
        .await
        .expect("the ended track closes after the grace period")
        .unwrap();
        tokio::time::resume();
        let received_after = within(async {
            send_subgroup_object(&client, &kept, &after).await;
            let mut receiver = streams.next().await.unwrap();
            read_subgroup_objects(&mut receiver).await
        })
        .await;

        // Assert
        assert_eq!(done.request_id(), ended_peer.request_id());
        assert_ne!(done.request_id(), kept_peer.request_id());
        assert_eq!(done.status_code(), PublishDoneCode::TrackEnded);
        assert_eq!(done.stream_count(), 0);
        assert_eq!(done.error_reason(), "finished");
        assert!(ended_object.is_none(), "{ended_object:?}");
        assert_eq!(received_before, vec![before]);
        assert_eq!(received_after, vec![after]);
    }

    #[tokio::test]
    async fn reverse_publish_responses_keep_metadata_and_objects_with_their_requests() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("publish-response-order").await;
        let (first_request, first_handler) = pending_publish(&client, &server, "first").await;
        let (second_request, second_handler) = pending_publish(&client, &server, "second").await;
        first_handler.accept_data_receiver().await;
        second_handler.accept_data_receiver().await;
        let first_object = Object {
            payload: Bytes::from_static(b"first publication"),
            ..object_with_extensions()
        };
        let second_object = Object {
            payload: Bytes::from_static(b"second publication"),
            ..object_with_extensions()
        };

        // Act
        let (first, second, first_peer, second_peer, first_received, second_received) =
            within(async {
                let second_peer = second_handler
                    .ok(17, FilterType::NextGroupStart)
                    .await
                    .unwrap();
                let second = second_request.await.unwrap().unwrap();
                let first_peer = first_handler
                    .ok(241, FilterType::LargestObject)
                    .await
                    .unwrap();
                let first = first_request.await.unwrap().unwrap();
                send_subgroup_object(&client, &second, &second_object).await;
                send_subgroup_object(&client, &first, &first_object).await;
                let mut first_receiver = first_subgroup_stream(&server, &first_peer).await;
                let first_received = read_subgroup_objects(&mut first_receiver).await;
                let mut second_receiver = first_subgroup_stream(&server, &second_peer).await;
                let second_received = read_subgroup_objects(&mut second_receiver).await;
                (
                    first,
                    second,
                    first_peer,
                    second_peer,
                    first_received,
                    second_received,
                )
            })
            .await;

        // Assert
        assert_eq!(first.request_id(), first_handler.request_id);
        assert_eq!(second.request_id(), second_handler.request_id);
        assert_ne!(first.request_id(), second.request_id());
        assert_eq!(first.track_alias(), first_handler.track_alias);
        assert_eq!(second.track_alias(), second_handler.track_alias);
        assert_ne!(first.track_alias(), second.track_alias());
        assert_eq!(first.track_name(), "first");
        assert_eq!(second.track_name(), "second");
        assert_eq!(first.subscriber_priority(), 241);
        assert_eq!(second.subscriber_priority(), 17);
        assert_eq!(first.filter_type(), FilterType::LargestObject);
        assert_eq!(second.filter_type(), FilterType::NextGroupStart);
        assert_eq!(first_peer.track_alias(), first.track_alias());
        assert_eq!(second_peer.track_alias(), second.track_alias());
        assert_eq!(first_received, vec![first_object]);
        assert_eq!(second_received, vec![second_object]);
    }
}
