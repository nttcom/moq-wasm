use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
};

use aws_lc_rs::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http_body_util::Full;
use hyper::{
    Request, Response, StatusCode, body::Bytes, header::AUTHORIZATION, server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use reqwest::Url;
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

pub(crate) const AUDIENCE: &str = "/projects/123/locations/region/services/console";
pub(crate) const NOW: u64 = 1_760_900_000;

pub(crate) struct TestIapKey {
    kid: String,
    pair: EcdsaKeyPair,
}

impl TestIapKey {
    pub(crate) fn new(kid: &str) -> Self {
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref()).unwrap();
        Self {
            kid: kid.into(),
            pair,
        }
    }

    fn jwk(&self) -> Value {
        let point = self.pair.public_key().as_ref();
        json!({
            "kid": self.kid,
            "kty": "EC",
            "crv": "P-256",
            "alg": "ES256",
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
        })
    }

    pub(crate) fn sign(&self, claims: &Value) -> String {
        self.sign_with_header(&json!({ "alg": "ES256", "kid": self.kid }), claims)
    }

    pub(crate) fn sign_with_header(&self, header: &Value, claims: &Value) -> String {
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature = self
            .pair
            .sign(&SystemRandom::new(), signing_input.as_bytes())
            .unwrap();
        format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        )
    }
}

pub(crate) fn iap_claims(sub: &str) -> Value {
    json!({
        "iss": "https://cloud.google.com/iap",
        "aud": AUDIENCE,
        "sub": sub,
        "email": "someone@example.com",
        "iat": NOW - 10,
        "exp": NOW + 590,
    })
}

pub(crate) struct FakeIapKeys {
    server: FakeHttpServer,
}

impl FakeIapKeys {
    pub(crate) async fn run(keys: &[&TestIapKey]) -> Self {
        Self::serve(keys.iter().map(|key| key.jwk()).collect()).await
    }

    pub(crate) async fn run_with_extra(keys: &[&TestIapKey], extra: Value) -> Self {
        let mut jwks: Vec<Value> = keys.iter().map(|key| key.jwk()).collect();
        jwks.push(extra);
        Self::serve(jwks).await
    }

    async fn serve(jwks: Vec<Value>) -> Self {
        let body = json!({ "keys": jwks }).to_string();
        Self {
            server: FakeHttpServer::run(StatusCode::OK, &body).await,
        }
    }

    pub(crate) fn url(&self) -> Url {
        Url::parse(&format!("{}/keys", self.server.url())).unwrap()
    }

    pub(crate) fn fetches(&self) -> usize {
        self.server.requests().len()
    }
}

pub(crate) struct SilentServer {
    url: Url,
    _accept_handle: JoinHandle<()>,
}

impl SilentServer {
    pub(crate) async fn run() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/keys", listener.local_addr().unwrap())).unwrap();
        Self {
            url,
            _accept_handle: tokio::spawn(hold_connections(listener)),
        }
    }

    pub(crate) fn url(&self) -> Url {
        self.url.clone()
    }
}

async fn hold_connections(listener: TcpListener) {
    let mut connections = Vec::new();
    loop {
        connections.push(listener.accept().await.unwrap().0);
    }
}

/// A request a fake server received: its path and query, and its
/// `Authorization` header.
pub(crate) type ReceivedRequest = (String, Option<String>);

/// Answers every request with `status` and `body` and records it.
pub(crate) struct FakeHttpServer {
    url: String,
    requests: Arc<Mutex<Vec<ReceivedRequest>>>,
    _accept_handle: JoinHandle<()>,
}

impl FakeHttpServer {
    pub(crate) async fn run(status: StatusCode, body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        Self {
            url,
            requests: requests.clone(),
            _accept_handle: tokio::spawn(serve_answer(
                listener,
                status,
                Bytes::from(body.to_string()),
                requests,
            )),
        }
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn requests(&self) -> Vec<ReceivedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

async fn serve_answer(
    listener: TcpListener,
    status: StatusCode,
    body: Bytes,
    requests: Arc<Mutex<Vec<ReceivedRequest>>>,
) {
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let body = body.clone();
        let requests = requests.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                let authorization = request
                    .headers()
                    .get(AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                requests
                    .lock()
                    .unwrap()
                    .push((request.uri().to_string(), authorization));
                let mut response = Response::new(Full::new(body.clone()));
                *response.status_mut() = status;
                async move { Ok::<_, Infallible>(response) }
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}
