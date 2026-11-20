use google_cloud::testing::FakeCloudStorage;

use crate::{app::App, location::Location, store::RegistryStore};

const BUCKET: &str = "registry-bucket";
const OBJECT: &str = "registry.json";

pub(crate) struct FakeRegistry {
    storage: FakeCloudStorage,
}

impl FakeRegistry {
    pub(crate) async fn run() -> Self {
        Self {
            storage: FakeCloudStorage::run(BUCKET).await,
        }
    }

    pub(crate) fn store(&self) -> RegistryStore {
        RegistryStore::new(Location::CloudStorage {
            endpoint: self.storage.endpoint(),
            bucket: BUCKET.into(),
            object: OBJECT.into(),
        })
        .unwrap()
    }

    pub(crate) fn put(&self, body: &serde_json::Value) {
        self.storage.put(OBJECT, serde_json::to_vec(body).unwrap());
    }

    pub(crate) fn body(&self) -> serde_json::Value {
        serde_json::from_slice(&self.storage.body(OBJECT).unwrap()).unwrap()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.storage.generation(OBJECT).unwrap()
    }
}

pub(crate) fn app(app_id: &str, sub: Option<&str>) -> App {
    App {
        app_id: app_id.into(),
        name: None,
        secret: format!("{app_id}-secret"),
        is_relay: false,
        sub: sub.map(Into::into),
        created_at: None,
        rotated_at: None,
        phone: None,
    }
}
