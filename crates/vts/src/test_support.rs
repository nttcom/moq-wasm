use serde_json::{Value, json};

use crate::{apps::Apps, apps::parse_apps, jwt::sign_token};

pub(crate) const APP_ID: &str = "APP";
pub(crate) const APP_SECRET: &str = "app-secret";
pub(crate) const RELAY_ID: &str = "RELAY";
pub(crate) const RELAY_SECRET: &str = "relay-secret";
pub(crate) const NOW: u64 = 1_759_600_000;

pub(crate) fn test_apps() -> Apps {
    let text = json!([
        { "appId": APP_ID, "secret": APP_SECRET, "isRelay": false },
        { "appId": RELAY_ID, "secret": RELAY_SECRET, "isRelay": true },
    ])
    .to_string();
    parse_apps(&text).unwrap()
}

pub(crate) fn app_claims() -> Value {
    json!({
        "appId": APP_ID,
        "publish": "site1",
        "subscribe": "site1",
        "iat": NOW,
        "exp": NOW + 3600,
    })
}

pub(crate) fn token(claims: &Value, secret: &str) -> String {
    sign_token(claims, secret).unwrap()
}
