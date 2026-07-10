use std::{collections::HashMap, sync::Arc};

use tokio::task::JoinHandle;

use crate::modules::{
    core::data_object::DataObject,
    relay::{
        cache::{duration::duration_from_env, track_cache::TrackCache},
        egress::coordinator::{EgressCommand, EgressFetchRequest},
        types::StreamSubgroupId,
    },
    session_repository::SessionRepository,
    types::SessionId,
};

const DATA_STREAM_INTERNAL_ERROR: u64 = 0x0;
const DEFAULT_FETCH_FILL_TIMEOUT_SECS: u64 = 20;

pub(crate) struct FetchIngestStart {
    pub(crate) upstream_publisher_session_id: SessionId,
    pub(crate) downstream_subscriber_session_id: SessionId,
    pub(crate) request_id: u64,
    pub(crate) fetch_handle: moqt::FetchHandle,
    pub(crate) cache: Arc<TrackCache>,
    pub(crate) requested_start: moqt::Location,
    pub(crate) requested_end: moqt::Location,
    pub(crate) egress_start: EgressFetchRequest,
}

pub(crate) struct FetchIngest {
    _join_handle: JoinHandle<()>,
}

impl FetchIngest {
    pub(crate) fn run(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        egress_sender: tokio::sync::mpsc::Sender<EgressCommand>,
        start: FetchIngestStart,
    ) -> Self {
        let downstream_subscriber_session_id = start.downstream_subscriber_session_id;
        let request_id = start.request_id;
        let join_handle = tokio::spawn(async move {
            if let Err(error) = Self::run_inner(session_repo.clone(), &egress_sender, start).await {
                tracing::error!(?error, "fetch ingest failed");
                Self::reset_downstream_fetch(
                    session_repo,
                    downstream_subscriber_session_id,
                    request_id,
                )
                .await;
            }
        });
        Self {
            _join_handle: join_handle,
        }
    }

    async fn run_inner(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        start: FetchIngestStart,
    ) -> anyhow::Result<()> {
        let timeout = Self::timeout();
        let result = tokio::time::timeout(
            timeout,
            Self::ingest_fetch_stream(session_repo.clone(), egress_sender, start),
        )
        .await;

        match result {
            Ok(result) => result,
            Err(_) => {
                anyhow::bail!("fetch ingest timed out after {:?}", timeout);
            }
        }
    }

    async fn ingest_fetch_stream(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        start: FetchIngestStart,
    ) -> anyhow::Result<()> {
        let mut subscriber = {
            let session_repo = session_repo.lock().await;
            session_repo
                .subscriber(start.upstream_publisher_session_id)
                .ok_or_else(|| anyhow::anyhow!("upstream publisher session not found"))?
        };
        let mut receiver = subscriber
            .create_fetch_receiver(&start.fetch_handle)
            .await?;
        let mut previous_object_ids = HashMap::<(u64, StreamSubgroupId), u64>::new();
        let start_eviction_generation = start.cache.eviction_generation();

        loop {
            match receiver.receive().await? {
                moqt::Fetch::Header(_) => {}
                moqt::Fetch::Object(object) => {
                    Self::append_fetch_object(
                        start.cache.clone(),
                        object,
                        &mut previous_object_ids,
                    )
                    .await?;
                }
                moqt::Fetch::End => {
                    if start.cache.eviction_generation() != start_eviction_generation {
                        tracing::warn!(
                            request_id = start.request_id,
                            "fetch fill crossed cache eviction; resetting downstream fetch"
                        );
                        anyhow::bail!("fetch fill crossed cache eviction");
                    }
                    start
                        .cache
                        .insert_fetch_known_range(start.requested_start, start.requested_end)
                        .await;
                    egress_sender
                        .send(EgressCommand::StartFetch(start.egress_start))
                        .await?;
                    return Ok(());
                }
            }
        }
    }

    async fn append_fetch_object(
        cache: Arc<TrackCache>,
        object: moqt::FetchObjectField,
        previous_object_ids: &mut HashMap<(u64, StreamSubgroupId), u64>,
    ) -> anyhow::Result<()> {
        let subgroup_id = StreamSubgroupId::Value(object.subgroup_id);
        let key = (object.group_id, subgroup_id.clone());

        let has_extensions = !object.extension_headers.prior_group_id_gap.is_empty()
            || !object.extension_headers.prior_object_id_gap.is_empty()
            || !object.extension_headers.immutable_extensions.is_empty();
        let header = moqt::SubgroupHeader::new(
            0,
            object.group_id,
            moqt::SubgroupId::Value(object.subgroup_id),
            object.publisher_priority,
            has_extensions,
            false,
        );
        let message_type = header.message_type;
        let object_id_delta = match previous_object_ids.get(&key) {
            Some(previous_object_id) => object
                .object_id
                .checked_sub(previous_object_id.saturating_add(1))
                .ok_or_else(|| anyhow::anyhow!("FETCH objects are not in ascending order"))?,
            None => object.object_id,
        };
        let subgroup_object = match object.fetch_object {
            moqt::FetchObject::Payload(payload) => moqt::SubgroupObject::new_payload(payload),
            moqt::FetchObject::Status(status) => {
                moqt::SubgroupObject::new_status(u8::from(status) as u64)
            }
        };

        cache
            .append_stream_object(
                object.group_id,
                &subgroup_id,
                None,
                DataObject::SubgroupHeader(header),
            )
            .await;
        cache
            .append_stream_object(
                object.group_id,
                &subgroup_id,
                Some(object.object_id),
                DataObject::SubgroupObject(moqt::SubgroupObjectField {
                    message_type,
                    object_id_delta,
                    extension_headers: object.extension_headers,
                    subgroup_object,
                }),
            )
            .await;
        previous_object_ids.insert(key, object.object_id);
        Ok(())
    }

    fn timeout() -> std::time::Duration {
        duration_from_env(
            "MOQT_FETCH_FILL_TIMEOUT_SECS",
            DEFAULT_FETCH_FILL_TIMEOUT_SECS,
        )
    }

    async fn reset_downstream_fetch(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        downstream_subscriber_session_id: SessionId,
        request_id: u64,
    ) {
        let publisher = {
            let session_repo = session_repo.lock().await;
            session_repo.publisher(downstream_subscriber_session_id)
        };
        let Some(publisher) = publisher else {
            return;
        };
        // FETCH_OK has already been sent. FIN would assert that every available object
        // was delivered, so failure after FETCH_OK must be signaled with RESET_STREAM.
        match publisher.new_fetch_sender(request_id).await {
            Ok(sender) => {
                if let Err(error) = sender.reset(DATA_STREAM_INTERNAL_ERROR).await {
                    tracing::error!(?error, "failed to reset downstream fetch stream");
                }
            }
            Err(error) => tracing::error!(?error, "failed to create downstream fetch stream"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::relay::cache::track_cache::FetchRangeResolution;
    use bytes::Bytes;
    use moqt::{ExtensionHeaders, FetchObject, FetchObjectField, ObjectStatus};

    #[tokio::test]
    async fn fetch_status_object_does_not_close_live_subgroup() {
        // Arrange: a FETCH response can contain EndOfGroup status, but that
        // must not close the shared live GroupCache.
        let cache = Arc::new(TrackCache::new());
        let mut previous_object_ids = HashMap::new();
        FetchIngest::append_fetch_object(
            cache.clone(),
            FetchObjectField::new(
                0,
                0,
                0,
                0,
                ExtensionHeaders {
                    prior_group_id_gap: vec![],
                    prior_object_id_gap: vec![],
                    immutable_extensions: vec![],
                },
                FetchObject::Status(ObjectStatus::EndOfGroup),
            ),
            &mut previous_object_ids,
        )
        .await
        .unwrap();
        FetchIngest::append_fetch_object(
            cache.clone(),
            FetchObjectField::new(
                1,
                0,
                0,
                0,
                ExtensionHeaders {
                    prior_group_id_gap: vec![],
                    prior_object_id_gap: vec![],
                    immutable_extensions: vec![],
                },
                FetchObject::Payload(Bytes::new()),
            ),
            &mut previous_object_ids,
        )
        .await
        .unwrap();

        // Act
        let resolution = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 1,
                    object_id: 1,
                },
            )
            .await;

        // Assert: only a completed FETCH known range may cover this historical gap.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }

    #[tokio::test]
    async fn partial_fetch_ingest_does_not_register_coverage() {
        // Arrange: a FETCH fill wrote an object but has not reached Fetch::End.
        let cache = Arc::new(TrackCache::new());
        let mut previous_object_ids = HashMap::new();
        FetchIngest::append_fetch_object(
            cache.clone(),
            FetchObjectField::new(
                0,
                0,
                0,
                0,
                ExtensionHeaders {
                    prior_group_id_gap: vec![],
                    prior_object_id_gap: vec![],
                    immutable_extensions: vec![],
                },
                FetchObject::Payload(Bytes::new()),
            ),
            &mut previous_object_ids,
        )
        .await
        .unwrap();

        // Act
        let resolution = cache
            .resolve_fetch_range(
                moqt::Location {
                    group_id: 0,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 1,
                },
            )
            .await;

        // Assert: only Fetch::End may register the filled range as known.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }
}
