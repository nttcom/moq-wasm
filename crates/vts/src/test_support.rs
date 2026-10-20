use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use app_registry::{Apps, Location, RegistryStore};
use auth_token::sign_token;
use serde_json::{Value, json};

pub(crate) const APP_ID: &str = "APP";
pub(crate) const APP_SECRET: &str = "app-secret";
pub(crate) const RELAY_ID: &str = "RELAY";
pub(crate) const RELAY_SECRET: &str = "relay-secret";
pub(crate) const NOW: u64 = 1_759_600_000;

pub(crate) fn test_apps_json() -> String {
    json!([
        { "appId": APP_ID, "secret": APP_SECRET, "isRelay": false },
        { "appId": RELAY_ID, "secret": RELAY_SECRET, "isRelay": true },
    ])
    .to_string()
}

pub(crate) fn test_apps() -> Apps {
    Apps::parse(&test_apps_json()).unwrap()
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

static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct TemporaryRegistryFile {
    path: PathBuf,
}

impl TemporaryRegistryFile {
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "vts-registry-{}-{}.json",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        Self { path }
    }

    pub(crate) fn store(&self) -> RegistryStore {
        RegistryStore::new(Location::File(self.path.clone())).unwrap()
    }

    pub(crate) fn write(&self, contents: &str) {
        std::fs::write(&self.path, contents).unwrap();
    }
}

impl Drop for TemporaryRegistryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
