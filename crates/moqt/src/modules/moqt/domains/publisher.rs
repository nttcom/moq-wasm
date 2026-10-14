use std::sync::Arc;

use crate::modules::moqt::control_plane::error_codes::{PublishDoneCode, RequestErrorCode};

use crate::{
    RequestFailure,
    modules::moqt::{
        control_plane::{
            control_messages::messages::{
                publish::Publish, publish_namespace::PublishNamespace,
                publish_namespace_done::PublishNamespaceDone,
            },
            enums::ResponseMessage,
            options::PublishOption,
        },
        data_plane::stream::{
            fetch_data_sender::FetchDataSender, stream_data_sender_factory::StreamDataSenderFactory,
        },
        domains::{
            pending_publish_namespace::PendingPublishNamespace,
            session_context::SessionContext,
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
        let vec_namespace: Vec<String> = namespace.split('/').map(|s| s.to_string()).collect();
        let request_id = self.session.control.next_request_id();
        let pending = self
            .session
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

    /// Withdraws a previous PUBLISH_NAMESPACE. Fire-and-forget: the spec
    /// defines no response message for PUBLISH_NAMESPACE_DONE.
    pub async fn publish_namespace_done(&self, namespace: String) -> anyhow::Result<()> {
        let vec_namespace = namespace.split('/').map(|s| s.to_string()).collect();
        self.session
            .control
            .notify(Notification::PublishNamespaceDone(
                PublishNamespaceDone::new(vec_namespace),
            ))
            .await?;
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
            .control
            .notify(Notification::PublishNamespaceCancel {
                namespace: namespace.split('/').map(|s| s.to_string()).collect(),
                code,
                reason: error_reason,
            })
            .await?;
        Ok(())
    }

    /// Ends a subscription this endpoint publishes on (draft-14 §9.12).
    pub async fn publish_done(
        &self,
        subscription: &Subscription,
        status_code: PublishDoneCode,
        stream_count: u64,
        error_reason: String,
    ) -> anyhow::Result<()> {
        self.session
            .control
            .notify(Notification::PublishDone {
                request_id: subscription.request_id(),
                code: status_code,
                stream_count,
                reason: error_reason,
            })
            .await?;
        Ok(())
    }

    pub async fn publish(
        &self,
        track_namespace: String,
        track_name: String,
        option: PublishOption,
    ) -> Result<Subscription, RequestFailure> {
        let track_alias = self.session.get_track_alias();
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let request_id = self.session.control.next_request_id();
        let content_exists = option.content_exists;
        let publish = Publish {
            request_id,
            track_namespace_tuple: vec_namespace,
            track_name: track_name.clone(),
            track_alias,
            group_order: option.group_order,
            content_exists,
            forward: option.forward,
            authorization_tokens: vec![],
            delivery_timeout: None,
            max_duration: option.max_cache_duration,
        };
        // A late PUBLISH_OK is discarded: the peer keeps subscription state
        // but receives no objects.
        let response = self
            .session
            .send_request(OutgoingRequest::Publish(publish))
            .await?
            .response()
            .await?;
        match response {
            ResponseMessage::PublishOk(message) => {
                if request_id != message.request_id {
                    Err(RequestFailure::ProtocolViolation(
                        "response for another request ID",
                    ))
                } else {
                    tracing::info!("Publish ok");
                    Ok(Subscription::PublisherInitiated(
                        PublisherInitiatedSubscription::new(
                            track_namespace,
                            track_name,
                            track_alias,
                            message,
                            option.max_cache_duration,
                        )
                        .with_content_exists(content_exists),
                    ))
                }
            }
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

    pub fn create_stream(&self, subscription: &Subscription) -> StreamDataSenderFactory {
        StreamDataSenderFactory::new(subscription.track_alias(), self.session.clone())
    }

    pub fn create_datagram(&self, subscription: &Subscription) -> DatagramSender {
        DatagramSender::new(subscription.track_alias(), self.session.clone())
    }

    pub async fn create_fetch_stream(&self, request_id: u64) -> anyhow::Result<FetchDataSender> {
        let send_stream = self.session.transport_connection.open_uni().await?;
        FetchDataSender::new(self.session.control.version(), send_stream, request_id).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::RequestErrorCode;

    use crate::{
        SessionEvent, modules::test_support::spawn_connected_dual_sessions, publish::PublishOption,
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
        let subscribe_ok_track_alias = session.get_track_alias();

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
}
