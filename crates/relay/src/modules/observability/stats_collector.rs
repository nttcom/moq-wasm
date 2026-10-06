use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use relay_stats::{
    ProcessStats, RelaySnapshot, SessionPeer as SnapshotPeer, SessionStats, SubscriptionStats,
    TrackStats,
};

use crate::modules::{
    cascading::inter_relay_connection_manager::InterRelayConnectionManager,
    data_plane::cache::{store::TrackCacheStore, track_cache::IngressCounters},
    domain::{
        pub_sub_directory::{DownstreamSubscriptionState, InMemoryLocalPubSubDirectory},
        session_id::SessionId,
        session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::session_repository::{SessionRepository, SessionState},
};

pub(crate) struct StatsSources {
    pub(crate) repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    pub(crate) directory: Arc<InMemoryLocalPubSubDirectory>,
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
}

pub(crate) struct StatsCollector {
    relay_id: String,
    sources: StatsSources,
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

impl StatsCollector {
    pub(crate) fn new(relay_id: String, sources: StatsSources) -> Self {
        Self { relay_id, sources }
    }

    pub(crate) fn relay_id(&self) -> &str {
        &self.relay_id
    }

    pub(crate) async fn collect(&self, timestamp_ms: u64, rss_bytes: Option<u64>) -> RelaySnapshot {
        let sessions = self.sources.repo.lock().await.session_states();
        let dialed_relay_ids = self
            .sources
            .inter_relay_connection_manager
            .dialed_relay_ids();
        let upstream_tracks = self.sources.directory.active_upstream_tracks();
        let ingress = self.take_ingress_counters(&upstream_tracks);
        let stats_publishers = self.stats_publishers(&upstream_tracks);
        let mut newest_publisher_of: HashMap<&TrackKey, SessionId> = HashMap::new();
        for (publisher_session_id, track_key) in &upstream_tracks {
            let newest = newest_publisher_of.entry(track_key).or_default();
            *newest = (*newest).max(*publisher_session_id);
        }
        let subscriptions = self
            .sources
            .directory
            .downstream_subscription_states()
            .into_iter()
            .map(|state| {
                let newest_received = ingress
                    .get(&state.track_key)
                    .and_then(|counters| counters.last_arrival);
                let publisher_session_id = newest_publisher_of
                    .get(&state.track_key)
                    .copied()
                    .unwrap_or_default();
                subscription_stats(state, publisher_session_id, newest_received)
            })
            .collect();
        let occupancy = self.sources.cache_store.occupancy();
        RelaySnapshot {
            relay_id: self.relay_id.clone(),
            timestamp_ms,
            process: ProcessStats {
                rss_bytes,
                cache_tracks: occupancy.tracks,
                cache_objects: occupancy.objects,
                cache_payload_bytes: occupancy.payload_bytes,
            },
            sessions: sessions
                .into_iter()
                .map(|session| {
                    let is_stats_publisher = stats_publishers.contains(&session.session_id);
                    let dialed_relay_id = dialed_relay_ids.get(&session.session_id).cloned();
                    session_stats(session, is_stats_publisher, dialed_relay_id)
                })
                .collect(),
            tracks: upstream_tracks
                .iter()
                .map(|(publisher_session_id, track_key)| {
                    track_stats(
                        *publisher_session_id,
                        track_key,
                        ingress.get(track_key).copied(),
                    )
                })
                .collect(),
            subscriptions,
        }
    }

    fn take_ingress_counters(
        &self,
        upstream_tracks: &[(SessionId, TrackKey)],
    ) -> HashMap<TrackKey, IngressCounters> {
        let track_keys: HashSet<&TrackKey> = upstream_tracks
            .iter()
            .map(|(_, track_key)| track_key)
            .collect();
        track_keys
            .into_iter()
            .filter_map(|track_key| {
                let cache = self.sources.cache_store.get(track_key)?;
                Some((track_key.clone(), cache.ingress_stats().take()))
            })
            .collect()
    }

    fn stats_publishers(&self, upstream_tracks: &[(SessionId, TrackKey)]) -> HashSet<SessionId> {
        let namespace = relay_stats::track_namespace(&self.relay_id);
        upstream_tracks
            .iter()
            .filter(|(_, track_key)| {
                track_key.track_namespace == namespace
                    && track_key.track_name == relay_stats::TRACK_NAME
            })
            .map(|(publisher_session_id, _)| *publisher_session_id)
            .collect()
    }
}

fn session_stats(
    session: SessionState,
    is_stats_publisher: bool,
    dialed_relay_id: Option<String>,
) -> SessionStats {
    let SessionState {
        session_id,
        peer,
        app_id,
        transport,
        addresses,
    } = session;
    SessionStats {
        session_id,
        peer: match peer {
            _ if is_stats_publisher => SnapshotPeer::StatsPublisher,
            SessionPeer::Client => SnapshotPeer::Client,
            SessionPeer::Relay => SnapshotPeer::Relay,
        },
        app_id,
        remote_address: addresses.remote.map(|address| address.to_string()),
        local_ip: addresses.local_ip.map(|ip| ip.to_string()),
        dialed_relay_id,
        rtt_us: micros(transport.rtt),
        current_mtu: transport.current_mtu,
        sent_bytes: transport.sent_bytes,
        sent_packets: transport.sent_packets,
        lost_packets: transport.lost_packets,
        cwnd: transport.cwnd,
        congestion_events: transport.congestion_events,
        sent_stream_data_blocked: transport.sent_stream_data_blocked,
        sent_data_blocked: transport.sent_data_blocked,
        received_stop_sending: transport.received_stop_sending,
        received_bytes: transport.received_bytes,
        received_stream_data_blocked: transport.received_stream_data_blocked,
        received_data_blocked: transport.received_data_blocked,
        received_reset_stream: transport.received_reset_stream,
    }
}

fn track_stats(
    publisher_session_id: SessionId,
    track_key: &TrackKey,
    ingress: Option<IngressCounters>,
) -> TrackStats {
    TrackStats {
        namespace: track_key.track_namespace.clone(),
        name: track_key.track_name.clone(),
        publisher_session_id,
        bytes_received: ingress.map_or(0, |counters| counters.bytes_received),
        max_arrival_gap_since_last_snapshot_us: ingress
            .map_or(0, |counters| micros(counters.max_arrival_gap)),
    }
}

fn subscription_stats(
    state: DownstreamSubscriptionState,
    publisher_session_id: SessionId,
    newest_received: Option<tokio::time::Instant>,
) -> SubscriptionStats {
    let DownstreamSubscriptionState {
        subscriber_session_id,
        request_id,
        track_key,
        delivery,
    } = state;
    let lag = match (newest_received, delivery.last_sent_received_at) {
        (Some(newest), Some(sent)) => newest.saturating_duration_since(sent),
        _ => Duration::ZERO,
    };
    SubscriptionStats {
        namespace: track_key.track_namespace,
        name: track_key.track_name,
        publisher_session_id,
        subscriber_session_id,
        request_id,
        bytes_sent: delivery.bytes_sent,
        streams_reset: delivery.streams_reset,
        lag_behind_newest_received_us: micros(lag),
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use bytes::Bytes;
    use moqt::ContentExists;
    use relay_stats::SessionPeer as SnapshotPeer;

    use super::{StatsCollector, StatsSources};
    use crate::modules::{
        auth::verified_token::VerifiedToken,
        cascading::inter_relay_connection_manager::InterRelayConnectionManager,
        data_plane::cache::store::TrackCacheStore,
        domain::{
            pub_sub_directory::{
                InMemoryLocalPubSubDirectory,
                entry::{ActiveUpstreamSubscription, UpstreamSubscriptionOrigin},
            },
            session_id::SessionId,
            session_peer::SessionPeer,
            track_key::TrackKey,
        },
        session::session_repository::{NewSession, SessionRepository},
        test_support::{
            mock_session::mock_session_with_transport_stats,
            relay_harness::fixtures::cached_object::{open_group, stream_object_with_payload},
        },
    };

    const RELAY_ID: &str = "relay-a";

    struct Fixture {
        repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        directory: Arc<InMemoryLocalPubSubDirectory>,
        cache_store: Arc<TrackCacheStore>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                repo: Arc::new(tokio::sync::Mutex::new(SessionRepository::new())),
                directory: Arc::new(InMemoryLocalPubSubDirectory::new()),
                cache_store: Arc::new(TrackCacheStore::new()),
            }
        }

        fn collector(&self) -> StatsCollector {
            let (session_event_sender, _) = tokio::sync::mpsc::unbounded_channel();
            StatsCollector::new(
                RELAY_ID.to_string(),
                StatsSources {
                    repo: self.repo.clone(),
                    directory: self.directory.clone(),
                    cache_store: self.cache_store.clone(),
                    inter_relay_connection_manager: Arc::new(InterRelayConnectionManager::new(
                        self.repo.clone(),
                        session_event_sender,
                        "relay-token".to_string(),
                    )),
                },
            )
        }

        async fn add_session(
            &self,
            session_id: SessionId,
            peer: SessionPeer,
            transport: moqt::TransportStats,
        ) {
            let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
            self.repo
                .lock()
                .await
                .add(
                    NewSession {
                        session_id,
                        session: mock_session_with_transport_stats(transport),
                        session_span: tracing::Span::none(),
                        peer,
                        verified_token: VerifiedToken::full_access(),
                    },
                    sender,
                )
                .await;
        }

        fn publish(
            &self,
            publisher_session_id: SessionId,
            namespace: &str,
            name: &str,
        ) -> TrackKey {
            let track_key = TrackKey::new(namespace, name);
            self.directory.register_upstream_subscription(
                track_key.clone(),
                publisher_session_id,
                ActiveUpstreamSubscription {
                    upstream_request_id: 0,
                    expires: None,
                    content_exists: ContentExists::False,
                    origin: UpstreamSubscriptionOrigin::Publish,
                    publisher_peer: SessionPeer::Client,
                },
            );
            track_key
        }
    }

    #[tokio::test]
    async fn sessions_carry_their_transport_stats_and_the_stats_publisher_is_marked() {
        // Arrange
        let fixture = Fixture::new();
        let client_transport = moqt::TransportStats {
            rtt: Duration::from_millis(12),
            lost_packets: 3,
            received_reset_stream: 2,
            ..moqt::TransportStats::default()
        };
        fixture
            .add_session(1, SessionPeer::Client, client_transport)
            .await;
        fixture
            .add_session(2, SessionPeer::Relay, moqt::TransportStats::default())
            .await;
        fixture.publish(
            2,
            &relay_stats::track_namespace(RELAY_ID),
            relay_stats::TRACK_NAME,
        );

        // Act
        let mut snapshot = fixture.collector().collect(1_000, Some(4_096)).await;

        // Assert
        snapshot.sessions.sort_by_key(|session| session.session_id);
        let [client, publisher] = &snapshot.sessions[..] else {
            panic!("expected two sessions, got {:?}", snapshot.sessions);
        };
        assert_eq!(client.peer, SnapshotPeer::Client);
        assert_eq!(client.rtt_us, 12_000);
        assert_eq!(client.lost_packets, 3);
        assert_eq!(client.received_reset_stream, 2);
        assert_eq!(publisher.peer, SnapshotPeer::StatsPublisher);
        assert_eq!(snapshot.relay_id, RELAY_ID);
        assert_eq!(snapshot.timestamp_ms, 1_000);
        assert_eq!(snapshot.process.rss_bytes, Some(4_096));
    }

    #[tokio::test(start_paused = true)]
    async fn tracks_and_subscriptions_report_their_traffic_and_the_delivery_lag() {
        // Arrange
        let fixture = Fixture::new();
        let track_key = fixture.publish(1, "app/live", "video");
        let cache = fixture
            .cache_store
            .get_or_create(&TrackKey::new("app/live", "video"));
        let open = open_group(&cache, 0, &[]);
        let first = stream_object_with_payload(0, 0, Bytes::from_static(b"1234"));
        let first_received_at = first.received_at;
        let _ = open.insert(first);
        tokio::time::advance(Duration::from_millis(250)).await;
        let _ = open.insert(stream_object_with_payload(0, 1, Bytes::from_static(b"56")));
        let signals = fixture
            .directory
            .register_downstream_subscription(9, 4, SessionPeer::Client, track_key, None)
            .unwrap();
        signals
            .delivery_stats
            .record_object_sent(4, first_received_at);

        // Act
        let snapshot = fixture.collector().collect(1_000, None).await;

        // Assert
        let [track] = &snapshot.tracks[..] else {
            panic!("expected one track, got {:?}", snapshot.tracks);
        };
        assert_eq!(
            (track.namespace.as_str(), track.name.as_str()),
            ("app/live", "video")
        );
        assert_eq!(track.bytes_received, 6);
        assert_eq!(track.max_arrival_gap_since_last_snapshot_us, 250_000);
        let [subscription] = &snapshot.subscriptions[..] else {
            panic!(
                "expected one subscription, got {:?}",
                snapshot.subscriptions
            );
        };
        assert_eq!(
            (subscription.subscriber_session_id, subscription.request_id),
            (9, 4)
        );
        assert_eq!(subscription.publisher_session_id, 1);
        assert_eq!(subscription.bytes_sent, 4);
        assert_eq!(subscription.lag_behind_newest_received_us, 250_000);
        assert_eq!(snapshot.process.cache_objects, 2);
        assert_eq!(snapshot.process.cache_payload_bytes, 6);
    }
}
