use anyhow::{Context, Result};
use moqt::{Session, TrackReader, subscribe::SubscribeOption};
use msf::Catalog;

use crate::selection::{Selection, TrackChoice, select_tracks};

/// Yields the catalog's current group from the joining FETCH
/// (draft-ietf-moq-msf-02 §5), then later objects from the subscription,
/// whose first stream arrives with the next catalog object, up to 30 s later.
pub struct CatalogReader {
    namespace: String,
    reader: TrackReader,
}

impl CatalogReader {
    pub async fn open(session: &Session, namespace: &str) -> Result<Self> {
        let (_, reader) = session
            .subscriber()
            .subscribe_current_group(
                namespace.to_string(),
                msf::CATALOG_TRACK_NAME.to_string(),
                SubscribeOption::default(),
            )
            .await
            .with_context(|| {
                format!(
                    "failed to subscribe {namespace}/{}",
                    msf::CATALOG_TRACK_NAME
                )
            })?;
        Ok(Self {
            namespace: namespace.to_string(),
            reader,
        })
    }

    /// `Ok(None)` once the catalog track has ended.
    pub async fn next(&mut self) -> Result<Option<Catalog>> {
        let Some(object) = self.reader.next_object().await? else {
            return Ok(None);
        };
        Catalog::parse(&object.payload)
            .map(Some)
            .context("catalog is not MSF JSON")
    }

    /// The tracks `choice` selects from the next catalog that lists any,
    /// skipping catalogs that list none; `Ok(None)` once the catalog track has
    /// ended. Fails as `next` and `select_tracks` do.
    pub async fn next_selection(&mut self, choice: TrackChoice<'_>) -> Result<Option<Selection>> {
        while let Some(catalog) = self.next().await? {
            if let Some(selection) = select_tracks(&catalog, choice)? {
                return Ok(Some(selection));
            }
            tracing::info!(
                track_namespace = self.namespace,
                "waiting for the catalog to list a selectable track"
            );
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use bytes::Bytes;
    use mediapack::loc::VideoCodec;
    use moqt::publish::{GroupBoundary, OutgoingObject, Retention};
    use msf::{KnownPackaging, KnownTrackRole, Track};

    use super::*;
    use crate::selection::VideoTrack;
    use crate::test_support::{FakePublisher, connect};

    const NAMESPACE: &str = "anon/room/alice";

    fn catalog(generated_at_ms: u64) -> Bytes {
        Bytes::from(serde_json::to_vec(&Catalog::new(Vec::new(), generated_at_ms)).unwrap())
    }

    fn catalog_with_video() -> Bytes {
        let mut track = Track::new("video", KnownPackaging::Loc, true);
        track.role = Some(KnownTrackRole::Video.into());
        track.codec = Some("avc1.42C01E".to_string());
        track.bitrate = Some(1_000_000);
        track.framerate = Some(30.0);
        Bytes::from(serde_json::to_vec(&Catalog::new(vec![track], 0)).unwrap())
    }

    async fn open_reader(publisher: &mut FakePublisher) -> (Arc<Session>, CatalogReader) {
        let session = connect(&publisher.url).await;
        let reader = CatalogReader::open(&session, NAMESPACE).await.unwrap();
        assert_eq!(publisher.next_served().await, msf::CATALOG_TRACK_NAME);
        (session, reader)
    }

    async fn next_selection(reader: &mut CatalogReader) -> Option<Selection> {
        tokio::time::timeout(
            Duration::from_secs(5),
            reader.next_selection(TrackChoice::default()),
        )
        .await
        .expect("no selection arrived")
        .unwrap()
    }

    async fn next_catalog(reader: &mut CatalogReader) -> Catalog {
        tokio::time::timeout(Duration::from_secs(5), reader.next())
            .await
            .expect("no catalog object arrived")
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn the_joined_group_is_read_from_the_fetch_then_from_the_subscription() {
        // Arrange
        let mut publisher = FakePublisher::spawn("catalog-reader-joined");
        let origin = publisher.origin.clone();
        origin
            .ensure_track(NAMESPACE, msf::CATALOG_TRACK_NAME, 0, Retention::NewestOnly)
            .await;
        let write = |group, payload| {
            origin.write(
                NAMESPACE,
                msf::CATALOG_TRACK_NAME,
                OutgoingObject::plain(group, payload),
            )
        };
        write(GroupBoundary::Next, catalog(1)).await.unwrap();
        let session = connect(&publisher.url).await;
        let mut reader = CatalogReader::open(&session, NAMESPACE).await.unwrap();
        assert_eq!(publisher.next_served().await, msf::CATALOG_TRACK_NAME);

        // Act
        let fetched = next_catalog(&mut reader).await;
        write(GroupBoundary::Within, catalog(2)).await.unwrap();
        let subscribed = next_catalog(&mut reader).await;

        // Assert
        assert_eq!(fetched, Catalog::new(Vec::new(), 1));
        assert_eq!(subscribed, Catalog::new(Vec::new(), 2));
        session.close().await;
    }

    #[tokio::test]
    async fn next_selection_skips_catalogs_that_list_no_selectable_track() {
        // Arrange
        let mut publisher = FakePublisher::spawn("catalog-reader-skips");
        let origin = publisher.origin.clone();
        origin
            .ensure_track(NAMESPACE, msf::CATALOG_TRACK_NAME, 0, Retention::NewestOnly)
            .await;
        let write = |group, payload| {
            origin.write(
                NAMESPACE,
                msf::CATALOG_TRACK_NAME,
                OutgoingObject::plain(group, payload),
            )
        };
        write(GroupBoundary::Next, catalog(1)).await.unwrap();
        let (session, mut reader) = open_reader(&mut publisher).await;

        // Act
        write(GroupBoundary::Within, catalog_with_video())
            .await
            .unwrap();
        let selection = next_selection(&mut reader).await;

        // Assert
        assert_eq!(
            selection,
            Some(Selection {
                video: Some(VideoTrack {
                    name: "video".to_string(),
                    codec: VideoCodec::H264,
                }),
                audio: None,
            })
        );
        session.close().await;
    }

    #[tokio::test]
    async fn next_selection_is_none_once_the_catalog_track_ends() {
        // Arrange
        let mut publisher = FakePublisher::spawn("catalog-reader-ends");
        let origin = publisher.origin.clone();
        origin
            .ensure_track(NAMESPACE, msf::CATALOG_TRACK_NAME, 0, Retention::NewestOnly)
            .await;
        let write = |group, payload| {
            origin.write(
                NAMESPACE,
                msf::CATALOG_TRACK_NAME,
                OutgoingObject::plain(group, payload),
            )
        };
        write(GroupBoundary::Next, catalog(1)).await.unwrap();
        let (session, mut reader) = open_reader(&mut publisher).await;

        // Act: a subscription stream lets PUBLISH_DONE end the track without the 10 s grace
        write(GroupBoundary::Within, catalog(2)).await.unwrap();
        origin
            .end_track(NAMESPACE, msf::CATALOG_TRACK_NAME)
            .await
            .unwrap();
        let selection = next_selection(&mut reader).await;

        // Assert
        assert_eq!(selection, None);
        session.close().await;
    }
}
