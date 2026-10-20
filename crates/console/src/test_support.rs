use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http_body_util::Full;
use hyper::{Request, Response, body::Bytes, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use reqwest::Url;
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
};
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
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random)
                .unwrap();
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
    url: Url,
    fetches: Arc<AtomicUsize>,
    _accept_handle: JoinHandle<()>,
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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/keys", listener.local_addr().unwrap())).unwrap();
        let body = Bytes::from(json!({ "keys": jwks }).to_string());
        let fetches = Arc::new(AtomicUsize::new(0));
        Self {
            url,
            fetches: fetches.clone(),
            _accept_handle: tokio::spawn(serve_keys(listener, body, fetches)),
        }
    }

    pub(crate) fn url(&self) -> Url {
        self.url.clone()
    }

    pub(crate) fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }
}

async fn serve_keys(listener: TcpListener, body: Bytes, fetches: Arc<AtomicUsize>) {
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let body = body.clone();
        let fetches = fetches.clone();
        tokio::spawn(async move {
            let service = service_fn(move |_: Request<hyper::body::Incoming>| {
                fetches.fetch_add(1, Ordering::SeqCst);
                let body = body.clone();
                async move { Ok::<_, Infallible>(Response::new(Full::new(body))) }
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}
