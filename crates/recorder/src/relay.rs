use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::{
    ClientConfig, DUAL, Endpoint, Session, Subscription, TerminationErrorCode, TrackObject,
    TrackReader,
    fetch::{Fetch, FetchDataReceiver, FetchOption},
    object::{ContentExists, ForwardingPreference, GroupOrder, Location, Object, ObjectStatus},
    subscribe::{FilterType, SubscribeOption},
};

const SUBSCRIBER_PRIORITY: u8 = 128;

/// The session follows the relay's GOAWAY by itself, so a recording outlives
/// a relay drain.
pub(crate) struct RelayConnection {
    pub(crate) session: Arc<Session>,
    endpoint: Endpoint<DUAL>,
}

impl RelayConnection {
    pub(crate) async fn connect(
        url: &str,
        verify_certificate: bool,
        authorization_token: Option<String>,
    ) -> Result<Self> {
        let endpoint = Endpoint::<DUAL>::create_client(&ClientConfig {
            port: 0,
            verify_certificate,
            authorization_token,
            follow_go_away: true,
        })?;
        tracing::info!(relay = url, "connecting to relay");
        let connecting = endpoint
            .connect(url)
            .await
            .with_context(|| format!("cannot connect to {url}"))?;
        let session = Arc::new(connecting.await?);
        Ok(Self { session, endpoint })
    }

    /// Waiting lets the CONNECTION_CLOSE reach the relay before the process
    /// exits.
    pub(crate) async fn close(&self) {
        self.session
            .close_with_error(TerminationErrorCode::NoError, "");
        self.endpoint.wait_idle().await;
    }
}

pub(crate) struct Track {
    pub(crate) subscription: Subscription,
    pub(crate) reader: TrackReader,
}

/// `NextGroupStart` begins a live video recording on a keyframe;
/// `LargestObject` continues right after what `fetch_cached` returns.
pub(crate) async fn subscribe(
    session: &Session,
    namespace: &str,
    name: &str,
    filter_type: FilterType,
) -> Result<Track> {
    let option = SubscribeOption {
        subscriber_priority: SUBSCRIBER_PRIORITY,
        group_order: GroupOrder::Ascending,
        forward: true,
        filter_type,
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

/// The last object of the group `track` joined at: the joining FETCH
/// with offset 0 that draft-ietf-moq-msf-01 §5 asks for beside a catalog
/// SUBSCRIBE. `None` when the publisher has sent nothing yet.
pub(crate) async fn fetch_joining_group(session: &Session, track: &Track) -> Result<Option<Bytes>> {
    let mut subscriber = session.subscriber();
    let handle = subscriber
        .fetch_relative_joining(&track.subscription, 0, FetchOption::default())
        .await?;
    let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
    let mut latest = None;
    loop {
        match receiver.receive().await? {
            Fetch::Object(object) if !object.payload.is_empty() => latest = Some(object.payload),
            Fetch::Object(_) => {}
            Fetch::End => return Ok(latest),
        }
    }
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

/// `None` for status objects, which carry no media.
pub(crate) fn fetched_track_object(object: Object) -> Option<TrackObject> {
    if object.status != ObjectStatus::Normal || object.payload.is_empty() {
        return None;
    }
    Some(TrackObject {
        group_id: object.location.group_id,
        subgroup_id: match object.forwarding {
            ForwardingPreference::Subgroup { subgroup_id } => subgroup_id,
            ForwardingPreference::Datagram => 0,
        },
        object_id: object.location.object_id,
        extension_headers: object.extension_headers,
        payload: object.payload,
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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
