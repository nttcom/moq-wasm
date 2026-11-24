use std::sync::Arc;

use broadcast::{Broadcast, RelayConnection};
use moqt::{
    client::{self, HandlerId, PublishTrackOptions, TrackWriter},
    executor,
    publish::Retention,
};
use tokio::sync::watch;

use super::{
    broadcast_end::BroadcastEnd,
    event_timeline::EventTimeline,
    media_source::{MediaSource, MediaSourceOptions},
};
use crate::Session;

/// A broadcast in `namespace`, published by `BroadcastBuilder::announce`.
pub fn create_broadcast(session: &Session, namespace: &[impl AsRef<str>]) -> BroadcastBuilder {
    let client = session.client().clone();
    let connection = RelayConnection::with_client_session(&client);
    let handler = client.add_handler(Arc::new(connection.clone()));
    let namespace = namespace
        .iter()
        .map(|part| part.as_ref().to_string())
        .collect();
    BroadcastBuilder {
        tracks: BroadcastTracks {
            client,
            broadcast: Broadcast::new(connection, namespace),
            handler,
        },
    }
}

/// The tracks registered before the PUBLISH_NAMESPACE, so that a SUBSCRIBE
/// the relay forwards at once finds them.
pub struct BroadcastBuilder {
    tracks: BroadcastTracks,
}

impl BroadcastBuilder {
    /// A track the session answers SUBSCRIBE and FETCH for. Fails when
    /// `options.publish` is set: a PUBLISH must follow PUBLISH_NAMESPACE, so
    /// publish such a track on the `LocalBroadcast`.
    pub async fn publish_track(
        &self,
        name: &str,
        options: PublishTrackOptions,
    ) -> anyhow::Result<TrackWriter> {
        anyhow::ensure!(
            !options.publish,
            "a PUBLISH follows the namespace: publish the track after announce"
        );
        self.tracks.publish_track(name, options).await
    }

    pub async fn event_timeline(
        &self,
        name: &str,
        retention: Retention,
    ) -> anyhow::Result<EventTimeline> {
        self.tracks.event_timeline(name, retention).await
    }

    /// PUBLISH_NAMESPACE (draft-14 §9.23). Fails when the relay refuses it.
    pub async fn announce(self) -> anyhow::Result<LocalBroadcast> {
        self.tracks.broadcast.announce().await?;
        Ok(LocalBroadcast {
            tracks: self.tracks,
            ended_sender: watch::Sender::new(false),
            withdrawn: false,
        })
    }
}

/// A published namespace; a media source's tracks reach subscribers through
/// the MSF catalog. `add_media_source` and dropping it without `close` spawn
/// tasks (the latter withdraws the namespace), so call both inside the tokio
/// runtime.
pub struct LocalBroadcast {
    tracks: BroadcastTracks,
    ended_sender: watch::Sender<bool>,
    withdrawn: bool,
}

impl LocalBroadcast {
    /// As `BroadcastBuilder::publish_track`, with `options.publish` too.
    pub async fn publish_track(
        &self,
        name: &str,
        options: PublishTrackOptions,
    ) -> anyhow::Result<TrackWriter> {
        self.tracks.publish_track(name, options).await
    }

    pub async fn event_timeline(
        &self,
        name: &str,
        retention: Retention,
    ) -> anyhow::Result<EventTimeline> {
        self.tracks.event_timeline(name, retention).await
    }

    pub fn add_media_source(&self, options: MediaSourceOptions) -> MediaSource {
        MediaSource::new(
            &self.tracks.broadcast,
            options,
            BroadcastEnd::new(&self.ended_sender),
        )
    }

    /// PUBLISH_NAMESPACE_DONE (draft-14 §9.26). The media sources stop:
    /// their pushes fail from now on.
    pub async fn close(mut self) -> anyhow::Result<()> {
        self.withdrawn = true;
        self.ended_sender.send_replace(true);
        self.tracks
            .client
            .moqt_session()
            .publisher()
            .publish_namespace_done(self.tracks.broadcast.namespace().join("/"))
            .await
    }
}

impl Drop for LocalBroadcast {
    fn drop(&mut self) {
        self.ended_sender.send_replace(true);
        if self.withdrawn {
            return;
        }
        let session = self.tracks.client.moqt_session().clone();
        let track_namespace = self.tracks.broadcast.namespace().join("/");
        executor::spawn("moq-wasm-namespace-done", async move {
            if let Err(error) = session
                .publisher()
                .publish_namespace_done(track_namespace.clone())
                .await
            {
                tracing::warn!(error = %format!("{error:#}"), track_namespace, "PUBLISH_NAMESPACE_DONE failed");
            }
        });
    }
}

/// What the builder and the broadcast share: the broadcast, answered by its
/// handler on the session until it is dropped.
struct BroadcastTracks {
    client: Arc<client::Session>,
    broadcast: Broadcast,
    handler: HandlerId,
}

impl LocalBroadcast {
    pub(crate) fn broadcast(&self) -> &Broadcast {
        &self.tracks.broadcast
    }
}

impl BroadcastTracks {
    async fn publish_track(
        &self,
        name: &str,
        options: PublishTrackOptions,
    ) -> anyhow::Result<TrackWriter> {
        self.client
            .publish_track(self.broadcast.namespace(), name, options)
            .await
    }

    async fn event_timeline(
        &self,
        name: &str,
        retention: Retention,
    ) -> anyhow::Result<EventTimeline> {
        let options = PublishTrackOptions {
            retention,
            ..PublishTrackOptions::default()
        };
        EventTimeline::open(self.publish_track(name, options).await?).await
    }
}

impl Drop for BroadcastTracks {
    fn drop(&mut self) {
        self.client.remove_handler(self.handler);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        media::MediaSourceError,
        test_support::{TEST_TIMEOUT, TestRelay},
    };

    #[tokio::test]
    async fn announce_publishes_the_namespace() {
        // Arrange
        let mut relay = TestRelay::spawn("announce");
        let session = relay.connect().await;
        let builder = create_broadcast(&session, &["anon", "robot"]);

        // Act
        let _broadcast = builder.announce().await.unwrap();

        // Assert
        assert_eq!(relay.next_request().await, "PUBLISH_NAMESPACE anon/robot");
    }

    #[tokio::test]
    async fn the_builder_refuses_a_track_published_before_the_namespace() {
        // Arrange
        let relay = TestRelay::spawn("builder-publish");
        let session = relay.connect().await;
        let builder = create_broadcast(&session, &["anon", "robot"]);
        let options = PublishTrackOptions {
            publish: true,
            ..PublishTrackOptions::default()
        };

        // Act
        let result = builder.publish_track("status", options).await;

        // Assert
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn close_withdraws_the_namespace() {
        // Arrange
        let mut relay = TestRelay::spawn("close");
        let session = relay.connect().await;
        let broadcast = create_broadcast(&session, &["anon", "robot"])
            .announce()
            .await
            .unwrap();
        relay.next_request().await;

        // Act
        broadcast.close().await.unwrap();

        // Assert
        assert_eq!(
            relay.next_request().await,
            "PUBLISH_NAMESPACE_DONE anon/robot"
        );
    }

    #[tokio::test]
    async fn dropping_the_broadcast_withdraws_the_namespace() {
        // Arrange
        let mut relay = TestRelay::spawn("drop");
        let session = relay.connect().await;
        let broadcast = create_broadcast(&session, &["anon", "robot"])
            .announce()
            .await
            .unwrap();
        relay.next_request().await;

        // Act
        drop(broadcast);

        // Assert
        assert_eq!(
            relay.next_request().await,
            "PUBLISH_NAMESPACE_DONE anon/robot"
        );
    }

    #[tokio::test]
    async fn a_media_source_stops_with_its_broadcast() {
        // Arrange
        let mut relay = TestRelay::spawn("media-stop");
        let session = relay.connect().await;
        let broadcast = create_broadcast(&session, &["anon", "robot"])
            .announce()
            .await
            .unwrap();
        relay.next_request().await;
        let source = broadcast.add_media_source(MediaSourceOptions::new("camera"));

        // Act
        broadcast.close().await.unwrap();
        let configured = source.configure_audio("opus", 48_000, 1, None);
        let waited = tokio::time::timeout(TEST_TIMEOUT, source.keyframe_request()).await;

        // Assert
        assert!(matches!(configured, Err(MediaSourceError::Stopped(_))));
        assert!(matches!(waited, Ok(Err(_))), "{waited:?}");
    }

    #[tokio::test]
    async fn an_event_timeline_appends_each_record_as_an_object() {
        // Arrange
        let mut relay = TestRelay::spawn("event-timeline");
        let session = relay.connect().await;
        let builder = create_broadcast(&session, &["anon", "robot"]);
        let events = builder
            .event_timeline("events", Retention::For(std::time::Duration::from_secs(60)))
            .await
            .unwrap();
        let _broadcast = builder.announce().await.unwrap();
        relay.next_request().await;

        // Act
        let object = relay
            .next_object_after("anon/robot", "events", async {
                events
                    .append(&json!({"T": 1, "data": "start"}))
                    .await
                    .unwrap();
            })
            .await;

        // Assert: §8 a record list of one
        assert_eq!(object.as_ref(), br#"[{"T":1,"data":"start"}]"#);
    }
}
