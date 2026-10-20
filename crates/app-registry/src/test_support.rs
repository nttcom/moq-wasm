use std::{
    convert::Infallible,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use http_body_util::{BodyExt, Full};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use reqwest::Url;
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{app::App, cloud_storage::Endpoint, location::Location, store::RegistryStore};

pub(crate) const BUCKET: &str = "registry-bucket";
pub(crate) const OBJECT: &str = "registry.json";

struct StoredObject {
    body: Vec<u8>,
    generation: u64,
}

pub(crate) struct FakeCloudStorage {
    url: Url,
    object: Arc<Mutex<Option<StoredObject>>>,
    _accept_handle: JoinHandle<()>,
}

impl FakeCloudStorage {
    pub(crate) async fn run() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let object = Arc::new(Mutex::new(None));
        Self {
            url,
            object: object.clone(),
            _accept_handle: tokio::spawn(accept_connections(listener, object)),
        }
    }

    pub(crate) fn store(&self) -> RegistryStore {
        RegistryStore::new(Location::CloudStorage {
            endpoint: Endpoint::Emulator(self.url.clone()),
            bucket: BUCKET.into(),
            object: OBJECT.into(),
        })
        .unwrap()
    }

    pub(crate) fn put(&self, body: &serde_json::Value) {
        let mut object = self.object.lock().unwrap();
        let generation = object.as_ref().map_or(1, |stored| stored.generation + 1);
        *object = Some(StoredObject {
            body: serde_json::to_vec(body).unwrap(),
            generation,
        });
    }

    pub(crate) fn body(&self) -> serde_json::Value {
        let object = self.object.lock().unwrap();
        serde_json::from_slice(&object.as_ref().unwrap().body).unwrap()
    }
}

pub(crate) struct SilentServer {
    url: Url,
    _accept_handle: JoinHandle<()>,
}

impl SilentServer {
    pub(crate) async fn run() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        Self {
            url,
            _accept_handle: tokio::spawn(hold_connections(listener)),
        }
    }

    pub(crate) fn store(&self) -> RegistryStore {
        RegistryStore::new(Location::CloudStorage {
            endpoint: Endpoint::Emulator(self.url.clone()),
            bucket: BUCKET.into(),
            object: OBJECT.into(),
        })
        .unwrap()
    }
}

async fn hold_connections(listener: TcpListener) {
    let mut connections = Vec::new();
    loop {
        connections.push(listener.accept().await.unwrap().0);
    }
}

async fn accept_connections(listener: TcpListener, object: Arc<Mutex<Option<StoredObject>>>) {
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let object = object.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| handle(request, object.clone()));
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

async fn handle(
    request: Request<Incoming>,
    object: Arc<Mutex<Option<StoredObject>>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let url = Url::parse(&format!("http://fake{}", request.uri())).unwrap();
    let query = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let object_path = format!("/storage/v1/b/{BUCKET}/o/{OBJECT}");
    let upload_path = format!("/upload/storage/v1/b/{BUCKET}/o");
    let method = request.method().clone();
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let mut object = object.lock().unwrap();

    let response = match (method, url.path()) {
        (Method::GET, path) if path == object_path => match (object.as_ref(), query("alt")) {
            (None, _) => respond(StatusCode::NOT_FOUND, Vec::new()),
            (Some(stored), Some(alt)) if alt == "media" => {
                let mut response = respond(StatusCode::OK, stored.body.clone());
                response
                    .headers_mut()
                    .insert("x-goog-generation", stored.generation.into());
                response
            }
            _ => respond(StatusCode::BAD_REQUEST, Vec::new()),
        },
        (Method::POST, path)
            if path == upload_path
                && query("uploadType").as_deref() == Some("media")
                && query("name").as_deref() == Some(OBJECT) =>
        {
            let current = object.as_ref().map_or(0, |stored| stored.generation);
            match query("ifGenerationMatch").map(|value| value.parse::<u64>()) {
                Some(Ok(expected)) if expected == current => {
                    *object = Some(StoredObject {
                        body: body.to_vec(),
                        generation: current + 1,
                    });
                    respond(StatusCode::OK, Vec::new())
                }
                Some(Ok(_)) => respond(StatusCode::PRECONDITION_FAILED, Vec::new()),
                _ => respond(StatusCode::BAD_REQUEST, Vec::new()),
            }
        }
        _ => respond(StatusCode::NOT_FOUND, Vec::new()),
    };
    Ok(response)
}

fn respond(status: StatusCode, body: Vec<u8>) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = status;
    response
}

pub(crate) fn app(app_id: &str, sub: Option<&str>) -> App {
    App {
        app_id: app_id.into(),
        name: None,
        secret: format!("{app_id}-secret"),
        is_relay: false,
        sub: sub.map(Into::into),
        created_at: None,
    }
}

static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct TemporaryRegistryFile {
    path: PathBuf,
}

impl TemporaryRegistryFile {
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "app-registry-{}-{}.json",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        Self { path }
    }

    pub(crate) fn store(&self) -> RegistryStore {
        RegistryStore::new(Location::File(self.path.clone())).unwrap()
    }

    pub(crate) fn put(&self, body: &serde_json::Value) {
        std::fs::write(&self.path, serde_json::to_vec(body).unwrap()).unwrap();
    }

    pub(crate) fn body(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(&self.path).unwrap()).unwrap()
    }
}

impl Drop for TemporaryRegistryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
