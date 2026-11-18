use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
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

use crate::cloud_storage::{CloudStorageBucket, Endpoint};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

struct StoredObject {
    body: Vec<u8>,
    generation: u64,
}

#[derive(Default)]
struct FakeState {
    objects: HashMap<String, StoredObject>,
    requests: Vec<String>,
    failure: Option<StatusCode>,
}

/// Serves the requests of `CloudStorageBucket` for one bucket from memory.
/// Each object's generation starts at 1 and rises by 1 per write.
pub struct FakeCloudStorage {
    url: Url,
    bucket: String,
    state: Arc<Mutex<FakeState>>,
    _accept_handle: JoinHandle<()>,
}

impl FakeCloudStorage {
    pub async fn run(bucket: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let state = Arc::new(Mutex::new(FakeState::default()));
        Self {
            url,
            bucket: bucket.to_owned(),
            state: state.clone(),
            _accept_handle: tokio::spawn(accept_connections(listener, bucket.to_owned(), state)),
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

    /// Each request as `<method> <path>?<query>`, still percent-encoded.
    pub fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().requests.clone()
    }

    /// Answers every later request with `status`.
    pub fn fail_with(&self, status: u16) {
        self.state.lock().unwrap().failure = Some(StatusCode::from_u16(status).unwrap());
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

    pub fn endpoint(&self) -> Endpoint {
        Endpoint::Emulator(self.url.clone())
    }
}

async fn hold_connections(listener: TcpListener) {
    let mut connections = Vec::new();
    loop {
        connections.push(listener.accept().await.unwrap().0);
    }
}

async fn accept_connections(listener: TcpListener, bucket: String, state: Arc<Mutex<FakeState>>) {
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let bucket = bucket.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| handle(request, bucket.clone(), state.clone()));
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

async fn handle(
    request: Request<Incoming>,
    bucket: String,
    state: Arc<Mutex<FakeState>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let url = Url::parse(&format!("http://fake{}", request.uri())).unwrap();
    let query = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let objects_path = format!("/storage/v1/b/{bucket}/o/");
    let upload_path = format!("/upload/storage/v1/b/{bucket}/o");
    let method = request.method().clone();
    let request_line = format!("{method} {}", request.uri());
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let mut state = state.lock().unwrap();
    state.requests.push(request_line);
    if let Some(status) = state.failure {
        return Ok(respond(status, Vec::new()));
    }
    let if_generation_match = query("ifGenerationMatch").map(|value| value.parse::<u64>());

    let response = match (method, url.path()) {
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
        (Method::DELETE, path) if path.starts_with(&objects_path) => {
            let object = percent_decode(&path[objects_path.len()..]);
            let current = state.objects.get(&object).map(|stored| stored.generation);
            match (current, if_generation_match) {
                (None, _) => respond(StatusCode::NOT_FOUND, Vec::new()),
                (Some(current), Some(Ok(expected))) if expected == current => {
                    state.objects.remove(&object);
                    respond(StatusCode::NO_CONTENT, Vec::new())
                }
                (Some(_), Some(Ok(_))) => respond(StatusCode::PRECONDITION_FAILED, Vec::new()),
                _ => respond(StatusCode::BAD_REQUEST, Vec::new()),
            }
        }
        (Method::POST, path)
            if path == upload_path && query("uploadType").as_deref() == Some("media") =>
        {
            let Some(object) = query("name") else {
                return Ok(respond(StatusCode::BAD_REQUEST, Vec::new()));
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
    };
    Ok(response)
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
