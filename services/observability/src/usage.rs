use serde::{Deserialize, Serialize, Serializer};

use crate::{
    clickhouse::ClickHouse,
    schema::{SESSION_STATS, SUBSCRIPTION_STATS, TRACK_STATS},
    series::{
        self, APP_CLIENT_SESSION, APP_NAMESPACE, RANGE, Series, SeriesQuery, SeriesTarget,
        app_client_sessions_sql,
    },
};

/// Cap on `Usage::namespaces`; the busiest are kept.
const MAX_NAMESPACES: u64 = 20;
/// Cap on `Usage::sessions`; the busiest are kept.
const MAX_SESSIONS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageQuery {
    pub app_id: String,
    pub from_ms: u64,
    pub to_ms: u64,
    pub points: u64,
}

/// An app's traffic in a range, counted on its client sessions only: a relay
/// session carries every app's traffic, and counting it too would count the
/// same bytes once per relay they cross. "Sent" is relay → client (the
/// subscribers' downlink), "received" client → relay (the publishers'
/// uplink).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Usage {
    /// `ingress_mbps`, `egress_mbps`, `sessions` and the other relay metrics,
    /// summed over the app's client sessions on every relay.
    pub traffic: Series,
    pub relays: Vec<RelayUsage>,
    pub namespaces: Vec<NamespaceUsage>,
    pub sessions: Vec<SessionUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayUsage {
    pub relay_id: String,
    pub sent_bytes: u64,
    pub received_bytes: u64,
    pub sessions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceUsage {
    pub namespace: String,
    /// Delivered to the app's subscribing clients.
    pub sent_bytes: u64,
    /// Received from the app's publishing clients.
    pub received_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUsage {
    pub relay_id: String,
    /// JavaScript numbers hold 53 bits, so the browser receives it as a string.
    #[serde(serialize_with = "as_string")]
    pub session_id: u64,
    pub sent_bytes: u64,
    pub received_bytes: u64,
    pub rtt_ms: f64,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
}

fn as_string<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

/// Each of the app's client sessions in the range with what its counters
/// grew by. A counter's first sample in the range is its base, so a session
/// that opens in the range loses what it moved before its first snapshot, at
/// most a second.
fn session_totals_sql(database: &str) -> String {
    format!(
        "SELECT relay_id, session_id, \
         max(sent_bytes) - min(sent_bytes) AS sent_bytes, \
         max(received_bytes) - min(received_bytes) AS received_bytes, \
         avg(rtt_us) / 1000 AS rtt_ms, \
         min(timestamp_ms) AS first_seen_ms, max(timestamp_ms) AS last_seen_ms \
         FROM {database}.{SESSION_STATS} \
         WHERE {RANGE} AND {APP_CLIENT_SESSION} \
         GROUP BY relay_id, session_id"
    )
}

fn sessions_sql(database: &str) -> String {
    format!(
        "{} ORDER BY sent_bytes + received_bytes DESC LIMIT {{limit:UInt64}}",
        session_totals_sql(database)
    )
}

fn relays_sql(database: &str) -> String {
    format!(
        "SELECT relay_id, sum(sent_bytes) AS sent_bytes, \
         sum(received_bytes) AS received_bytes, count() AS sessions \
         FROM ({}) GROUP BY relay_id ORDER BY relay_id",
        session_totals_sql(database)
    )
}

/// What each namespace delivered to the app's subscribing clients, per
/// subscription, and received from its publishing clients, per track, the
/// busiest first.
fn namespaces_sql(database: &str) -> String {
    let clients = app_client_sessions_sql(database);
    format!(
        "SELECT namespace, sum(sent) AS sent_bytes, sum(received) AS received_bytes FROM ( \
         SELECT namespace, toUInt64(max(bytes_sent) - min(bytes_sent)) AS sent, toUInt64(0) AS received \
         FROM {database}.{SUBSCRIPTION_STATS} \
         WHERE {RANGE} AND {APP_NAMESPACE} AND (relay_id, subscriber_session_id) IN ({clients}) \
         GROUP BY relay_id, subscriber_session_id, request_id, namespace \
         UNION ALL \
         SELECT namespace, toUInt64(0) AS sent, toUInt64(max(bytes_received) - min(bytes_received)) AS received \
         FROM {database}.{TRACK_STATS} \
         WHERE {RANGE} AND {APP_NAMESPACE} AND (relay_id, publisher_session_id) IN ({clients}) \
         GROUP BY relay_id, publisher_session_id, namespace, name) \
         GROUP BY namespace ORDER BY sent_bytes + received_bytes DESC, namespace \
         LIMIT {{namespaces:UInt64}}"
    )
}

pub async fn usage(clickhouse: &ClickHouse, query: &UsageQuery) -> anyhow::Result<Usage> {
    let database = clickhouse.database();
    let params = [
        ("app", query.app_id.clone()),
        ("from", query.from_ms.to_string()),
        ("to", query.to_ms.to_string()),
        ("limit", MAX_SESSIONS.to_string()),
        ("namespaces", MAX_NAMESPACES.to_string()),
    ];
    let traffic_query = SeriesQuery {
        app_id: query.app_id.clone(),
        relay_id: None,
        target: SeriesTarget::App,
        from_ms: query.from_ms,
        to_ms: query.to_ms,
        points: query.points,
    };
    let (relays_sql, sessions_sql, namespaces_sql) = (
        relays_sql(database),
        sessions_sql(database),
        namespaces_sql(database),
    );
    let (traffic, relays, sessions, namespaces) = tokio::try_join!(
        series::query(clickhouse, &traffic_query),
        clickhouse.select::<RelayUsage>(&relays_sql, &params),
        clickhouse.select::<SessionUsage>(&sessions_sql, &params),
        clickhouse.select::<NamespaceUsage>(&namespaces_sql, &params),
    )?;
    Ok(Usage {
        traffic,
        relays,
        namespaces,
        sessions,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{SessionUsage, namespaces_sql, sessions_sql};

    #[test]
    fn namespaces_count_only_traffic_with_the_apps_clients() {
        // Act
        let sql = namespaces_sql("obs");

        // Assert
        assert_eq!(
            sql.matches("IN (SELECT relay_id, session_id").count(),
            2,
            "{sql}"
        );
        assert!(
            sql.contains("(relay_id, subscriber_session_id) IN"),
            "{sql}"
        );
        assert!(sql.contains("(relay_id, publisher_session_id) IN"), "{sql}");
    }

    #[test]
    fn sessions_are_the_apps_client_sessions_busiest_first() {
        // Act
        let sql = sessions_sql("obs");

        // Assert
        assert!(
            sql.contains("peer = 'client' AND app_id = {app:String}"),
            "{sql}"
        );
        assert!(
            sql.ends_with("ORDER BY sent_bytes + received_bytes DESC LIMIT {limit:UInt64}"),
            "{sql}"
        );
    }

    #[test]
    fn a_session_id_reaches_the_browser_unrounded() {
        // Arrange
        let session = SessionUsage {
            relay_id: "relay-a".to_string(),
            session_id: 1_791_278_627_397_623_670,
            sent_bytes: 1,
            received_bytes: 2,
            rtt_ms: 3.0,
            first_seen_ms: 4,
            last_seen_ms: 5,
        };

        // Act / Assert
        assert_eq!(
            serde_json::to_value(&session).unwrap()["session_id"],
            json!("1791278627397623670")
        );
    }
}
