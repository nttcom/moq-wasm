use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use http_body_util::{BodyExt, Full};
use http_server::HttpServer;
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
};
use reqwest::Url;
use tokio::{net::TcpListener, task::JoinHandle};

use crate::cloud_storage::{CloudStorageBucket, Endpoint};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

struct StoredObject {
    body: Vec<u8>,
    generation: u64,
}

#[derive(Default)]
struct FakeState {
    objects: HashMap<String, StoredObject>,
}

/// Serves the requests of `CloudStorageBucket` for one bucket from memory.
/// Each object's generation starts at 1 and rises by 1 per write.
pub struct FakeCloudStorage {
    url: Url,
    bucket: String,
    state: Arc<Mutex<FakeState>>,
    _http: HttpServer,
}

impl FakeCloudStorage {
    pub async fn run(bucket: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let state = Arc::new(Mutex::new(FakeState::default()));
        let handler_bucket = bucket.to_owned();
        let handler_state = state.clone();
        Self {
            url,
            bucket: bucket.to_owned(),
            state,
            _http: HttpServer::run(listener, move |request, _peer| {
                handle(request, handler_bucket.clone(), handler_state.clone())
            }),
        }
    }

    pub fn endpoint(&self) -> Endpoint {
        Endpoint::Emulator(self.url.clone())
    }

    pub fn bucket(&self) -> CloudStorageBucket {
        CloudStorageBucket::new(self.endpoint(), &self.bucket, REQUEST_TIMEOUT).unwrap()
    }

    /// Writes `object` without a generation condition, raising its generation by 1.
    pub fn put(&self, object: &str, body: impl Into<Vec<u8>>) {
        let mut state = self.state.lock().unwrap();
        let generation = state
            .objects
            .get(object)
            .map_or(1, |stored| stored.generation + 1);
        state.objects.insert(
            object.to_owned(),
            StoredObject {
                body: body.into(),
                generation,
            },
        );
    }

    pub fn body(&self, object: &str) -> Option<Vec<u8>> {
        let state = self.state.lock().unwrap();
        state.objects.get(object).map(|stored| stored.body.clone())
    }

    pub fn generation(&self, object: &str) -> Option<u64> {
        let state = self.state.lock().unwrap();
        state.objects.get(object).map(|stored| stored.generation)
    }
}

/// Accepts connections and never answers on them.
pub struct SilentServer {
    url: Url,
    _accept_handle: JoinHandle<()>,
}

impl SilentServer {
    pub async fn run() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        Self {
            url,
            _accept_handle: tokio::spawn(hold_connections(listener)),
        }
    }

    pub fn url(&self) -> Url {
        self.url.clone()
    }

    pub fn endpoint(&self) -> Endpoint {
        Endpoint::Emulator(self.url())
    }
}

async fn hold_connections(listener: TcpListener) {
    let mut connections = Vec::new();
    loop {
        connections.push(listener.accept().await.unwrap().0);
    }
}

async fn handle(
    request: Request<Incoming>,
    bucket: String,
    state: Arc<Mutex<FakeState>>,
) -> Response<Full<Bytes>> {
    let url = Url::parse(&format!("http://fake{}", request.uri())).unwrap();
    let query = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let objects_path = format!("/storage/v1/b/{bucket}/o/");
    let upload_path = format!("/upload/storage/v1/b/{bucket}/o");
    let method = request.method().clone();
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let mut state = state.lock().unwrap();
    let if_generation_match = query("ifGenerationMatch").map(|value| value.parse::<u64>());

    match (method, url.path()) {
        (Method::GET, path) if path.starts_with(&objects_path) => {
            let object = percent_decode(&path[objects_path.len()..]);
            match (state.objects.get(&object), query("alt")) {
                (None, _) => respond(StatusCode::NOT_FOUND, Vec::new()),
                (Some(stored), Some(alt)) if alt == "media" => {
                    let mut response = respond(StatusCode::OK, stored.body.clone());
                    response
                        .headers_mut()
                        .insert("x-goog-generation", stored.generation.into());
                    response
                }
                _ => respond(StatusCode::BAD_REQUEST, Vec::new()),
            }
        }
        (Method::POST, path)
            if path == upload_path && query("uploadType").as_deref() == Some("media") =>
        {
            let Some(object) = query("name") else {
                return respond(StatusCode::BAD_REQUEST, Vec::new());
            };
            let current = state
                .objects
                .get(&object)
                .map_or(0, |stored| stored.generation);
            match if_generation_match {
                Some(Ok(expected)) if expected == current => {
                    state.objects.insert(
                        object,
                        StoredObject {
                            body: body.to_vec(),
                            generation: current + 1,
                        },
                    );
                    respond(StatusCode::OK, Vec::new())
                }
                Some(Ok(_)) => respond(StatusCode::PRECONDITION_FAILED, Vec::new()),
                _ => respond(StatusCode::BAD_REQUEST, Vec::new()),
            }
        }
        _ => respond(StatusCode::NOT_FOUND, Vec::new()),
    }
}

/// Decodes the `%XX` escapes of an object name. Read as a query value, a `+`
/// would become a space; no test names an object with one.
fn percent_decode(segment: &str) -> String {
    let url = Url::parse(&format!("http://fake/?name={segment}")).unwrap();
    url.query_pairs().next().unwrap().1.into_owned()
}

fn respond(status: StatusCode, body: Vec<u8>) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = status;
    response
}
