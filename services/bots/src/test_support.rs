use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use app_registry::testing::TemporaryRegistryFile;
use google_cloud::MetadataToken;
use serde_json::json;
use url::Url;

use http_server::{HttpServer, json_response};
use hyper::StatusCode;
use tokio::net::TcpListener;

use crate::{bot_types::BotTypes, bots::Bots, signer::BotTokenSigner, store::BotStore};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn temporary_store() -> BotStore {
    let dir = std::env::temp_dir().join(format!(
        "bots-store-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    BotStore::open(&Url::from_directory_path(dir).unwrap()).unwrap()
}

/// `Bots` with one type, `recorder`, launched as `/usr/bin/true`, at most
/// two at once, running 60 s at most, for the app `app` of the registry.
pub(crate) struct TestBots {
    pub(crate) bots: Bots,
    pub(crate) store: BotStore,
    _registry: TemporaryRegistryFile,
}

pub(crate) fn test_bots() -> TestBots {
    let types = BotTypes::parse(
        r#"{"recorder": {"launcher": {"kind": "local", "program": "/usr/bin/true"},
                         "args": ["record"], "grace_seconds": 30, "max_seconds": 60, "max_running": 2}}"#,
        &Arc::new(MetadataToken::new(reqwest::Client::new())),
    )
    .unwrap();
    let registry = TemporaryRegistryFile::new();
    registry.put(&json!([{ "appId": "app", "secret": "app-secret", "isRelay": false }]));
    let store = temporary_store();
    let bots = Bots::new(
        types,
        store.clone(),
        BotTokenSigner::new(registry.store(), None),
        None,
        "http://bots.test".to_string(),
    );
    TestBots {
        bots,
        store,
        _registry: registry,
    }
}

/// A VTS that verifies every token as the app `app`'s launch token for its
/// whole root, signed now and living a minute.
pub(crate) struct FakeVts {
    pub(crate) url: String,
    _server: HttpServer,
}

pub(crate) async fn fake_vts() -> FakeVts {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/verify", listener.local_addr().unwrap());
    let server = HttpServer::run(listener, |_, _| async {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        json_response(
            StatusCode::OK,
            &json!({ "appId": "app", "isRelay": false,
                     "claims": { "appId": "app", "bots": "", "iat": now, "exp": now + 60 } }),
        )
    });
    FakeVts {
        url,
        _server: server,
    }
}
