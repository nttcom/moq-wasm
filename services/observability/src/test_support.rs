use telemetry::relay_stats::{ProcessStats, RelaySnapshot};
use tokio::net::TcpListener;

use crate::{
    api_server::{ApiServer, ApiState},
    clickhouse::ClickHouse,
    config::ClickHouseConfig,
};

/// An API server on a free port whose ClickHouse refuses every connection,
/// with the server's base URL.
pub(crate) async fn api_server(state: impl FnOnce(ClickHouse) -> ApiState) -> (String, ApiServer) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let clickhouse = ClickHouse::new(ClickHouseConfig {
        url: "http://127.0.0.1:1".to_string(),
        database: "observability".to_string(),
        user: None,
        password: None,
    });
    (url, ApiServer::run(listener, state(clickhouse)))
}

pub(crate) fn snapshot(timestamp_ms: u64) -> RelaySnapshot {
    RelaySnapshot {
        relay_id: "relay-a".to_string(),
        timestamp_ms,
        process: ProcessStats::default(),
        sessions: vec![],
        tracks: vec![],
        subscriptions: vec![],
    }
}

/// The VTS `/verify` answer for a token of `app_id` issued a minute ago that
/// lives another hour.
pub(crate) fn vts_answer(app_id: &str, subscribe: Option<&str>) -> String {
    let now = auth_token::unix_now();
    serde_json::json!({
        "appId": app_id,
        "isRelay": false,
        "claims": { "appId": app_id, "subscribe": subscribe, "iat": now - 60, "exp": now + 3600 },
    })
    .to_string()
}
