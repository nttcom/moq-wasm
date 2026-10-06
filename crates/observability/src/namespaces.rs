use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    clickhouse::ClickHouse,
    schema::{SESSION_STATS, SUBSCRIPTION_STATS, TRACK_STATS},
};

const MAX_NAMESPACES: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceSummary {
    pub namespace: String,
    pub tracks: Vec<String>,
    pub relays: Vec<String>,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
    #[serde(default)]
    pub subscriptions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishedNamespaces {
    pub namespaces: Vec<NamespaceSummary>,
    pub subscriptions: u64,
    pub clients: u64,
}

#[derive(Deserialize)]
struct SubscriptionCount {
    namespace: String,
    subscriptions: u64,
}

#[derive(Deserialize)]
struct ClientCount {
    clients: u64,
}

pub struct NamespaceQuery {
    pub from_ms: u64,
    pub to_ms: u64,
    pub app_id: Option<String>,
}

pub async fn published_namespaces(
    clickhouse: &ClickHouse,
    query: &NamespaceQuery,
) -> anyhow::Result<PublishedNamespaces> {
    let database = clickhouse.database();
    let params = [
        ("from", query.from_ms.to_string()),
        ("to", query.to_ms.to_string()),
        ("app", query.app_id.clone().unwrap_or_default()),
        (
            "excluded_prefix",
            format!("{}/", relay_stats::NAMESPACE_ROOT),
        ),
        ("excluded_app", relay_stats::NAMESPACE_ROOT.to_string()),
        ("limit", MAX_NAMESPACES.to_string()),
    ];
    let range = "timestamp_ms >= {from:UInt64} AND timestamp_ms <= {to:UInt64}";
    let in_app = "({app:String} = '' OR namespace = {app:String} \
                  OR startsWith(namespace, concat({app:String}, '/')))";
    let client_sessions = format!(
        "SELECT relay_id, session_id FROM {database}.{SESSION_STATS} \
         WHERE {range} AND peer = 'client' AND app_id != {{excluded_app:String}} \
         AND ({{app:String}} = '' OR app_id = {{app:String}})"
    );
    let namespaces_sql = format!(
        "SELECT namespace, groupUniqArray(name) AS tracks, groupUniqArray(relay_id) AS relays, \
         min(timestamp_ms) AS first_seen_ms, max(timestamp_ms) AS last_seen_ms \
         FROM {database}.{TRACK_STATS} \
         WHERE {range} AND {in_app} AND NOT startsWith(namespace, {{excluded_prefix:String}}) \
         GROUP BY namespace ORDER BY last_seen_ms DESC LIMIT {{limit:UInt64}}"
    );
    let subscriptions_sql = format!(
        "SELECT namespace, uniqExact(relay_id, subscriber_session_id, request_id) AS subscriptions \
         FROM {database}.{SUBSCRIPTION_STATS} \
         WHERE {range} AND {in_app} AND (relay_id, subscriber_session_id) IN ({client_sessions}) \
         GROUP BY namespace"
    );
    let clients_sql =
        format!("SELECT uniqExact(relay_id, session_id) AS clients FROM ({client_sessions})");
    let (mut namespaces, counts, clients) = tokio::try_join!(
        clickhouse.select::<NamespaceSummary>(&namespaces_sql, &params),
        clickhouse.select::<SubscriptionCount>(&subscriptions_sql, &params),
        clickhouse.select::<ClientCount>(&clients_sql, &params),
    )?;
    let counts: HashMap<String, u64> = counts
        .into_iter()
        .map(|count| (count.namespace, count.subscriptions))
        .collect();
    for summary in &mut namespaces {
        summary.tracks.sort();
        summary.relays.sort();
        summary.subscriptions = counts.get(&summary.namespace).copied().unwrap_or(0);
    }
    Ok(PublishedNamespaces {
        subscriptions: counts.values().sum(),
        clients: clients.first().map_or(0, |count| count.clients),
        namespaces,
    })
}
