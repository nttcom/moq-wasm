use std::sync::Arc;

use anyhow::{Context, Result};
use moqt::{
    CertificateVerification, ClientConfig, Endpoint, NATIVE, Session, Subscription, TrackReader,
    fetch::{FetchDataReceiver, FetchOption},
    object::{ContentExists, Location},
    subscribe::{FilterType, SubscribeOption},
};

use crate::token_refresh_task::{TokenRefreshTask, TokenRenewal};

/// The token a recording or a replay presents to the relay: the one the
/// caller of the API sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceToken {
    /// `None` only on a server started with `--insecure-skip-auth`.
    pub(crate) token: Option<String>,
    /// The token mint that renews `token`, a temporary app's. Any other token
    /// is not renewed, and the relay ends the session when it expires.
    pub(crate) issuer_url: Option<String>,
}

/// The session follows the relay's GOAWAY by itself, so a recording outlives
/// a relay drain.
pub(crate) struct RelayConnection {
    pub(crate) session: Arc<Session>,
    _token_refresh: Option<TokenRefreshTask>,
}

impl RelayConnection {
    pub(crate) async fn connect(
        url: &str,
        verify_certificate: bool,
        authorization_token: Option<String>,
    ) -> Result<Self> {
        let endpoint = Endpoint::<NATIVE>::create_client(&ClientConfig {
            certificate_verification: CertificateVerification::native_roots_or_disabled(
                verify_certificate,
            ),
            authorization_token: authorization_token.map(Into::into),
            ..ClientConfig::default()
        })?;
        tracing::info!(relay = url, "connecting to relay");
        let connecting = endpoint
            .connect(url)
            .await
            .with_context(|| format!("cannot connect to {url}"))?;
        let session = Arc::new(connecting.await?);
        Ok(Self {
            session,
            _token_refresh: None,
        })
    }

    pub(crate) async fn connect_service(
        url: &str,
        verify_certificate: bool,
        namespace: &str,
        token: &ServiceToken,
    ) -> Result<Self> {
        let ServiceToken { token, issuer_url } = token.clone();
        let mut connection = Self::connect(url, verify_certificate, token.clone()).await?;
        if let (Some(issuer_url), Some(token)) = (issuer_url, token) {
            connection._token_refresh = Some(TokenRefreshTask::run(
                connection.session.clone(),
                TokenRenewal::Issuer(issuer_url),
                namespace.split('/').next().unwrap_or_default().to_string(),
                token,
            ));
        }
        Ok(connection)
    }

    pub(crate) async fn close(&self) {
        self.session.close().await;
    }
}

/// 2 seconds of video at 30 fps, the time within which the relay passes on
/// only one request for a new group.
const FETCHED_GROUP_OBJECTS: u64 = 60;

pub(crate) struct Track {
    pub(crate) subscription: Subscription,
    pub(crate) reader: TrackReader,
}

/// `LargestObject` continues right after what `fetch_cached` returns.
pub(crate) async fn subscribe(
    session: &Session,
    namespace: &str,
    name: &str,
    filter_type: FilterType,
) -> Result<Track> {
    let option = SubscribeOption {
        filter_type,
        ..SubscribeOption::default()
    };
    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(namespace.to_string(), name.to_string(), option)
        .await
        .with_context(|| format!("failed to subscribe {namespace}/{name}"))?;
    Ok(Track {
        reader: subscriber.track_reader(&subscription),
        subscription,
    })
}

/// Begins a live video recording at the first object of a group: the
/// current group when it began within `FETCHED_GROUP_OBJECTS`, else a new
/// group the publisher is asked to start. See
/// `moqt::Subscriber::subscribe_group_start`.
pub(crate) async fn subscribe_group_start(
    session: &Session,
    namespace: &str,
    name: &str,
) -> Result<Track> {
    let (subscription, reader) = session
        .subscriber()
        .subscribe_group_start(
            namespace.to_string(),
            name.to_string(),
            FETCHED_GROUP_OBJECTS,
            SubscribeOption::default().subscriber_priority,
        )
        .await
        .with_context(|| format!("failed to subscribe {namespace}/{name}"))?;
    Ok(Track {
        reader,
        subscription,
    })
}

/// An audio object decodes alone, and a meeting participant who sends the
/// audio as a single stream never starts a new group, so `NextGroupStart`
/// (draft-ietf-moq-transport-14 §9.7) would deliver none of it. Live or after
/// `fetch_cached`.
pub(crate) async fn subscribe_audio(
    session: &Session,
    namespace: &str,
    name: &str,
) -> Result<Track> {
    subscribe(session, namespace, name, FilterType::LargestObject).await
}

/// A standalone FETCH from the track's first location through the largest
/// object the track's subscription reported (draft-14 §9.16.1: End Location is the last
/// object plus one), so a `LargestObject` subscription continues where it
/// ends. The relay serves what it caches and asks the publisher for the
/// rest. `None` when the subscription reported no content.
pub(crate) async fn fetch_cached(
    session: &Session,
    track: &Track,
) -> Result<Option<FetchDataReceiver>> {
    let subscription = &track.subscription;
    let ContentExists::True { location: largest } = subscription.content_exists() else {
        return Ok(None);
    };
    let mut subscriber = session.subscriber();
    let handle = subscriber
        .fetch(
            subscription.track_namespace().to_string(),
            subscription.track_name().to_string(),
            Location {
                group_id: 0,
                object_id: 0,
            },
            Location {
                group_id: largest.group_id,
                object_id: largest.object_id + 1,
            },
            FetchOption::default(),
        )
        .await?;
    Ok(Some(subscriber.accept_fetch_receiver(&handle).await?))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use moqt::publish::{GroupBoundary, OutgoingObject, Retention};

    use super::*;
    use crate::test_support::FakePublisher;

    const NAMESPACE: &str = "anon/room/alice";
    const AUDIO: &str = "audio_64kbps";

    #[tokio::test]
    async fn an_audio_subscription_continues_the_group_the_publisher_keeps_open() {
        // Arrange: like a single-stream audio track, the publisher never starts a second group
        let mut publisher = FakePublisher::spawn();
        let origin = publisher.origin.clone();
        origin
            .ensure_track(NAMESPACE, AUDIO, 0, Retention::NewestOnly)
            .await;
        let write = |group, payload| {
            origin.write(
                NAMESPACE,
                AUDIO,
                OutgoingObject::plain(group, Bytes::from_static(payload)),
            )
        };
        write(GroupBoundary::Next, b"o0").await.unwrap();
        let connection = RelayConnection::connect(&publisher.url, false, None)
            .await
            .unwrap();

        // Act
        let mut track = subscribe_audio(&connection.session, NAMESPACE, AUDIO)
            .await
            .unwrap();
        assert_eq!(publisher.served.recv().await.as_deref(), Some(AUDIO));
        write(GroupBoundary::Within, b"o1").await.unwrap();
        let object = tokio::time::timeout(Duration::from_secs(5), track.reader.next_object())
            .await
            .expect("no audio object arrived")
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!((object.group_id, object.object_id), (0, 1));
        assert_eq!(object.payload, &b"o1"[..]);
        connection.close().await;
    }
}
