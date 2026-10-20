use std::{
    convert::Infallible,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use app_registry::RegistryStore;
use http_body_util::Full;
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{CONTENT_TYPE, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::identity::Identity;

pub struct ConsoleState {
    pub identity: Identity,
    pub store: RegistryStore,
}

pub struct ConsoleServer {
    _accept_handle: JoinHandle<()>,
}

impl ConsoleServer {
    pub fn run(listener: TcpListener, state: Arc<ConsoleState>) -> Self {
        Self {
            _accept_handle: tokio::spawn(accept_connections(listener, state)),
        }
    }
}

async fn accept_connections(listener: TcpListener, state: Arc<ConsoleState>) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "accept failed");
                continue;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| handle(request, state.clone()));
            if let Err(error) = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                tracing::debug!(%peer, %error, "connection closed with error");
            }
        });
    }
}

type JsonResponse = Response<Full<Bytes>>;

async fn handle(
    request: Request<Incoming>,
    state: Arc<ConsoleState>,
) -> Result<JsonResponse, Infallible> {
    let (head, _body) = request.into_parts();
    let response = match (head.method, head.uri.path()) {
        (Method::GET, "/api/me") => me(&head.headers, &state).await,
        _ => error_response(StatusCode::NOT_FOUND, "not_found"),
    };
    Ok(response)
}

async fn me(headers: &HeaderMap, state: &ConsoleState) -> JsonResponse {
    let sub = match state.identity.sub(headers, unix_now()).await {
        Ok(sub) => sub,
        Err(error) => {
            tracing::info!(error = format!("{error:#}"), "request not authenticated");
            return error_response(StatusCode::UNAUTHORIZED, "unauthenticated");
        }
    };
    let snapshot = match state.store.load().await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::warn!(error = format!("{error:#}"), "registry read failed");
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable");
        }
    };
    let app = snapshot.apps.find_by_sub(&sub).map(|app| {
        json!({
            "appId": app.app_id,
            "name": app.name,
            "createdAt": app.created_at,
        })
    });
    json_response(StatusCode::OK, json!({ "app": app }))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn error_response(status: StatusCode, error: &str) -> JsonResponse {
    json_response(status, json!({ "error": error }))
}

fn json_response(status: StatusCode, body: Value) -> JsonResponse {
    let mut response = Response::new(Full::from(body.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

#[cfg(test)]
mod tests {
    use app_registry::testing::TemporaryRegistryFile;
    use reqwest::Url;

    use super::*;
    use crate::{
        identity::{IAP_ASSERTION_HEADER, IdentitySettings},
        test_support::{AUDIENCE, FakeIapKeys, TestIapKey, iap_claims},
    };

    struct TestConsole {
        base_url: String,
        registry_file: TemporaryRegistryFile,
        _server: ConsoleServer,
    }

    async fn start_console(identity: Identity) -> TestConsole {
        let registry_file = TemporaryRegistryFile::new();
        registry_file.put(&json!([
            { "appId": "app-1", "name": "camera", "secret": "s", "isRelay": false,
              "sub": "accounts.google.com:1", "createdAt": 1760900000 },
            { "appId": "relay", "secret": "r", "isRelay": true },
        ]));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(ConsoleState {
            identity,
            store: registry_file.store(),
        });
        TestConsole {
            base_url,
            registry_file,
            _server: ConsoleServer::run(listener, state),
        }
    }

    fn development(sub: &str) -> Identity {
        Identity::Development { sub: sub.into() }
    }

    async fn get_me(console: &TestConsole, assertion: Option<&str>) -> reqwest::Response {
        let mut request = reqwest::Client::new().get(format!("{}/api/me", console.base_url));
        if let Some(assertion) = assertion {
            request = request.header(IAP_ASSERTION_HEADER, assertion);
        }
        request.send().await.unwrap()
    }

    #[tokio::test]
    async fn me_returns_the_app_of_the_sub_without_its_secret() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = get_me(&console, None).await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({ "app": { "appId": "app-1", "name": "camera", "createdAt": 1760900000 } })
        );
    }

    #[tokio::test]
    async fn me_returns_no_app_for_a_sub_without_one() {
        // Arrange
        let console = start_console(development("accounts.google.com:2")).await;

        // Act
        let response = get_me(&console, None).await;

        // Assert
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({ "app": null })
        );
    }

    #[tokio::test]
    async fn me_identifies_the_sub_from_a_valid_iap_assertion() {
        // Arrange
        let key = TestIapKey::new("key-1");
        let keys = FakeIapKeys::run(&[&key]).await;
        let identity = Identity::new(
            IdentitySettings {
                iap_audience: Some(AUDIENCE.into()),
                development_sub: None,
                on_cloud_run: true,
            },
            keys.url(),
        )
        .unwrap();
        let console = start_console(identity).await;
        let mut claims = iap_claims("accounts.google.com:1");
        claims["iat"] = json!(unix_now());
        claims["exp"] = json!(unix_now() + 600);

        // Act
        let response = get_me(&console, Some(&key.sign(&claims))).await;

        // Assert
        assert_eq!(
            response.json::<Value>().await.unwrap()["app"]["appId"],
            "app-1"
        );
    }

    #[tokio::test]
    async fn me_rejects_a_request_without_an_iap_assertion() {
        // Arrange
        let identity = Identity::new(
            IdentitySettings {
                iap_audience: Some(AUDIENCE.into()),
                development_sub: None,
                on_cloud_run: true,
            },
            Url::parse("http://127.0.0.1:9/keys").unwrap(),
        )
        .unwrap();
        let console = start_console(identity).await;

        // Act
        let response = get_me(&console, None).await;

        // Assert
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_answers_503_when_the_registry_cannot_be_read() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;
        console.registry_file.write("not a registry");

        // Act
        let response = get_me(&console, None).await;

        // Assert
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
