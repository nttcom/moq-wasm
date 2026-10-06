use std::sync::Arc;

use anyhow::bail;

use crate::{
    DatagramSender,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    publish::Publish, publish_done::PublishDone,
                    publish_namespace::PublishNamespace,
                    publish_namespace_cancel::PublishNamespaceCancel,
                    publish_namespace_done::PublishNamespaceDone,
                },
            },
            enums::ResponseMessage,
            options::PublishOption,
        },
        data_plane::{
            object::fetch::FetchHeader,
            stream::{
                fetch_data_sender::FetchDataSender,
                stream_data_sender_factory::StreamDataSenderFactory,
            },
        },
        domains::{
            pending_publish_namespace::PendingPublishNamespace,
            session_context::{LateResponseAction, SessionContext},
            subscription::{PublisherInitiatedSubscription, Subscription},
        },
    },
    wire::RequestError,
};

pub struct Publisher {
    pub(crate) session: Arc<SessionContext>,
}

impl Publisher {
    pub async fn publish_namespace(&self, namespace: String) -> anyhow::Result<()> {
        self.begin_publish_namespace(namespace)
            .await?
            .accepted()
            .await
    }

    pub async fn begin_publish_namespace(
        &self,
        namespace: String,
    ) -> anyhow::Result<PendingPublishNamespace> {
        let vec_namespace: Vec<String> = namespace.split('/').map(|s| s.to_string()).collect();
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session.get_request_id();
        let registered_sender = self.session.register_response_sender(
            request_id,
            sender,
            LateResponseAction::PublishNamespaceDone {
                namespace: vec_namespace.clone(),
            },
        );
        let publish_namespace = PublishNamespace::new(request_id, vec_namespace, vec![]);
        self.session
            .send_stream
            .send(
                ControlMessageType::PublishNamespace,
                publish_namespace.encode(),
            )
            .await?;
        Ok(PendingPublishNamespace {
            session: self.session.clone(),
            request_id,
            receiver,
            _registered_sender: registered_sender,
        })
    }

    /// Withdraws a previous PUBLISH_NAMESPACE. Fire-and-forget: the spec
    /// defines no response message for PUBLISH_NAMESPACE_DONE.
    pub async fn publish_namespace_done(&self, namespace: String) -> anyhow::Result<()> {
        let vec_namespace = namespace.split('/').map(|s| s.to_string()).collect();
        let publish_namespace_done = PublishNamespaceDone::new(vec_namespace);
        self.session
            .send_stream
            .send(
                ControlMessageType::PublishNamespaceDone,
                publish_namespace_done.encode(),
            )
            .await?;
        Ok(())
    }

    /// Tells the peer this side no longer serves `namespace` (draft-14
    /// §9.6). Fire-and-forget: the draft defines no response.
    pub async fn publish_namespace_cancel(
        &self,
        namespace: String,
        error_code: u64,
        error_reason: String,
    ) -> anyhow::Result<()> {
        let vec_namespace = namespace.split('/').map(|s| s.to_string()).collect();
        let publish_namespace_cancel =
            PublishNamespaceCancel::new(vec_namespace, error_code, error_reason);
        self.session
            .send_stream
            .send(
                ControlMessageType::PublishNamespaceCancel,
                publish_namespace_cancel.encode(),
            )
            .await?;
        Ok(())
    }

    pub async fn publish_done(
        &self,
        request_id: u64,
        status_code: u64,
        stream_count: u64,
        error_reason: String,
    ) -> anyhow::Result<()> {
        let publish_done = PublishDone::new(request_id, status_code, stream_count, error_reason);
        self.session
            .send_stream
            .send(ControlMessageType::PublishDone, publish_done.encode())
            .await?;
        Ok(())
    }

    pub async fn publish(
        &self,
        track_namespace: String,
        track_name: String,
        option: PublishOption,
    ) -> anyhow::Result<Subscription> {
        let track_alias = self.session.get_track_alias();
        let vec_namespace = track_namespace.split('/').map(|s| s.to_string()).collect();
        let (sender, receiver) = tokio::sync::oneshot::channel::<ResponseMessage>();
        let request_id = self.session.get_request_id();
        // A late PUBLISH_OK is discarded: the peer keeps subscription state
        // but receives no objects; see LateResponseAction::Discard for why
        // no PUBLISH_DONE is sent yet.
        let _registered_sender =
            self.session
                .register_response_sender(request_id, sender, LateResponseAction::Discard);
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
            max_duration: None,
        };
        let bytes = publish.encode();
        self.session
            .send_stream
            .send(ControlMessageType::Publish, bytes)
            .await?;
        let response = self.session.await_response(receiver).await?;
        match response {
            ResponseMessage::PublishOk(message) => {
                if request_id != message.request_id {
                    bail!("Protocol violation")
                } else {
                    tracing::info!("Publish ok");
                    Ok(Subscription::PublisherInitiated(
                        PublisherInitiatedSubscription::new(
                            track_namespace,
                            track_name,
                            track_alias,
                            message,
                        )
                        .with_content_exists(content_exists),
                    ))
                }
            }
            ResponseMessage::PublishError(request_id, error_code, reason_phrase) => {
                tracing::info!("Publish error");
                Err(RequestError {
                    request_id,
                    error_code,
                    reason_phrase,
                }
                .into())
            }
            _ => bail!("Protocol violation"),
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
        FetchDataSender::new(send_stream, FetchHeader::new(request_id)).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{
        PublishOption, SessionEvent, modules::test_support::spawn_connected_dual_sessions,
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
            .publish_namespace_cancel("live/room".to_string(), 0x5, "moved".to_string())
            .await
            .unwrap();

        // Assert
        let SessionEvent::PublishNamespaceCancel(cancel) = server.receive_event().await.unwrap()
        else {
            panic!("expected PUBLISH_NAMESPACE_CANCEL from the client");
        };
        assert_eq!(cancel.track_namespace(), "live/room");
        assert_eq!(cancel.error_code(), 0x5);
        assert_eq!(cancel.error_reason(), "moved");
    }
}
