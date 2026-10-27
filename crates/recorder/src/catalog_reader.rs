use anyhow::{Context, Result};
use moqt::Session;
use msf::Catalog;

use crate::relay::Track;

/// Yields the catalog's current group from the joining FETCH
/// (draft-ietf-moq-msf-01 §5), then later objects from the subscription,
/// whose first stream arrives with the next catalog object, up to 30 s later.
pub(crate) struct CatalogReader {
    track: Track,
}

impl CatalogReader {
    pub(crate) async fn open(session: &Session, namespace: &str) -> Result<Self> {
        let (subscription, reader) = session
            .subscriber()
            .subscribe_current_group(namespace.to_string(), msf::CATALOG_TRACK_NAME.to_string())
            .await
            .with_context(|| {
                format!(
                    "failed to subscribe {namespace}/{}",
                    msf::CATALOG_TRACK_NAME
                )
            })?;
        Ok(Self {
            track: Track {
                subscription,
                reader,
            },
        })
    }

    /// `Ok(None)` once the catalog track has ended.
    pub(crate) async fn next(&mut self) -> Result<Option<Catalog>> {
        let Some(object) = self.track.reader.next_object().await? else {
            return Ok(None);
        };
        serde_json::from_slice(&object.payload)
            .map(Some)
            .context("catalog is not MSF JSON")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use moqt::publish::{GroupBoundary, OutgoingObject, Retention};

    use super::*;
    use crate::{relay::RelayConnection, test_support::FakePublisher};

    const NAMESPACE: &str = "anon/room/alice";

    fn catalog(generated_at_ms: u64) -> Bytes {
        Bytes::from(serde_json::to_vec(&Catalog::new(Vec::new(), generated_at_ms)).unwrap())
    }

    #[tokio::test]
    async fn a_catalog_object_added_to_the_joined_group_is_read() {
        // Arrange: `Origin` refuses the joining FETCH, so only the subscription delivers
        let mut publisher = FakePublisher::spawn();
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
        let connection = RelayConnection::connect(&publisher.url, false, None)
            .await
            .unwrap();
        let mut reader = CatalogReader::open(&connection.session, NAMESPACE)
            .await
            .unwrap();
        assert_eq!(
            publisher.served.recv().await.as_deref(),
            Some(msf::CATALOG_TRACK_NAME)
        );

        // Act
        write(GroupBoundary::Within, catalog(2)).await.unwrap();
        let read = tokio::time::timeout(Duration::from_secs(5), reader.next())
            .await
            .expect("no catalog object arrived")
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!(read, Catalog::new(Vec::new(), 2));
        connection.close().await;
    }
}
