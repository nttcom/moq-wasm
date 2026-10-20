use std::{
    convert::Infallible,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use app_registry::{App, RegistryStore};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{CONTENT_TYPE, HOST, HeaderValue, ORIGIN},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use reqwest::Url;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{
    apps::{self, Outcome},
    identity::Identity,
    input::{CreateAppRequest, InvalidInput, IssueTokenRequest},
};

pub const CSRF_HEADER: &str = "x-console-request";
const MAX_BODY_BYTES: usize = 16 * 1024;

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

struct Refusal {
    status: StatusCode,
    error: &'static str,
}

impl Refusal {
    fn response(self) -> JsonResponse {
        error_response(self.status, self.error)
    }
}

async fn handle(
    request: Request<Incoming>,
    state: Arc<ConsoleState>,
) -> Result<JsonResponse, Infallible> {
    let (head, body) = request.into_parts();
    if head.method != Method::GET && !is_same_origin_request(&head.headers) {
        return Ok(error_response(StatusCode::FORBIDDEN, "forbidden"));
    }
    let headers = &head.headers;
    let response = match (head.method.clone(), head.uri.path()) {
        (Method::GET, "/api/me") => me(headers, &state).await,
        (Method::POST, "/api/app") => create_app(headers, body, &state).await,
        (Method::POST, "/api/tokens") => issue_token(headers, body, &state).await,
        (Method::POST, "/api/app/secret") => rotate_secret(headers, &state).await,
        (Method::DELETE, "/api/app") => delete_app(headers, &state).await,
        _ => error_response(StatusCode::NOT_FOUND, "not_found"),
    };
    Ok(response)
}

async fn me(headers: &HeaderMap, state: &ConsoleState) -> JsonResponse {
    let sub = match authenticate(headers, state).await {
        Ok(sub) => sub,
        Err(refusal) => return refusal.response(),
    };
    match state.store.load().await {
        Ok(snapshot) => {
            let app = snapshot
                .apps
                .find_by_sub(&sub)
                .filter(|app| !app.is_relay)
                .map(app_json);
            json_response(StatusCode::OK, json!({ "app": app }))
        }
        Err(error) => registry_unavailable(&error),
    }
}

async fn create_app(headers: &HeaderMap, body: Incoming, state: &ConsoleState) -> JsonResponse {
    let sub = match authenticate(headers, state).await {
        Ok(sub) => sub,
        Err(refusal) => return refusal.response(),
    };
    let request: CreateAppRequest = match read_json(body).await {
        Ok(request) => request,
        Err(refusal) => return refusal.response(),
    };
    let name = match request.name() {
        Ok(name) => name,
        Err(invalid) => return invalid_input(invalid),
    };
    respond(
        apps::create_app(&state.store, &sub, name, unix_now()).await,
        |app| {
            tracing::info!(app_id = %app.app_id, "app created");
            json_response(StatusCode::CREATED, json!({ "app": app_json(&app) }))
        },
    )
}

async fn issue_token(headers: &HeaderMap, body: Incoming, state: &ConsoleState) -> JsonResponse {
    let sub = match authenticate(headers, state).await {
        Ok(sub) => sub,
        Err(refusal) => return refusal.response(),
    };
    let request: IssueTokenRequest = match read_json(body).await {
        Ok(request) => request,
        Err(refusal) => return refusal.response(),
    };
    let scope = match request.scope() {
        Ok(scope) => scope,
        Err(invalid) => return invalid_input(invalid),
    };
    respond(
        apps::issue_token(&state.store, &sub, scope, unix_now()).await,
        |issued| {
            tracing::info!(expires_at = issued.expires_at, "token issued");
            json_response(
                StatusCode::OK,
                json!({ "token": issued.token, "expiresAt": issued.expires_at }),
            )
        },
    )
}

async fn rotate_secret(headers: &HeaderMap, state: &ConsoleState) -> JsonResponse {
    let sub = match authenticate(headers, state).await {
        Ok(sub) => sub,
        Err(refusal) => return refusal.response(),
    };
    respond(apps::rotate_secret(&state.store, &sub).await, |app| {
        tracing::info!(app_id = %app.app_id, "secret rotated");
        json_response(StatusCode::OK, json!({ "app": app_json(&app) }))
    })
}

async fn delete_app(headers: &HeaderMap, state: &ConsoleState) -> JsonResponse {
    let sub = match authenticate(headers, state).await {
        Ok(sub) => sub,
        Err(refusal) => return refusal.response(),
    };
    respond(apps::delete_app(&state.store, &sub).await, |()| {
        tracing::info!("app deleted");
        empty_response(StatusCode::NO_CONTENT)
    })
}

async fn authenticate(headers: &HeaderMap, state: &ConsoleState) -> Result<String, Refusal> {
    state
        .identity
        .sub(headers, unix_now())
        .await
        .map_err(|error| {
            tracing::info!(error = format!("{error:#}"), "request not authenticated");
            Refusal {
                status: StatusCode::UNAUTHORIZED,
                error: "unauthenticated",
            }
        })
}

fn is_same_origin_request(headers: &HeaderMap) -> bool {
    if !headers.contains_key(CSRF_HEADER) {
        return false;
    }
    let Some(origin) = headers.get(ORIGIN) else {
        return true;
    };
    let host = headers.get(HOST).and_then(|host| host.to_str().ok());
    let origin = origin
        .to_str()
        .ok()
        .and_then(|origin| Url::parse(origin).ok());
    match (origin, host) {
        (Some(origin), Some(host)) => authority(&origin).is_some_and(|authority| authority == host),
        _ => false,
    }
}

fn authority(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

async fn read_json<T: DeserializeOwned>(body: Incoming) -> Result<T, Refusal> {
    let invalid_request = || Refusal {
        status: StatusCode::BAD_REQUEST,
        error: "invalid_request",
    };
    let bytes = Limited::new(body, MAX_BODY_BYTES)
        .collect()
        .await
        .map_err(|_| invalid_request())?
        .to_bytes();
    serde_json::from_slice(&bytes).map_err(|_| invalid_request())
}

fn respond<T>(
    result: anyhow::Result<Outcome<T>>,
    done: impl FnOnce(T) -> JsonResponse,
) -> JsonResponse {
    match result {
        Ok(Outcome::Done(value)) => done(value),
        Ok(Outcome::NoApp) => error_response(StatusCode::NOT_FOUND, "no_app"),
        Ok(Outcome::AlreadyHasApp) => error_response(StatusCode::CONFLICT, "app_exists"),
        Err(error) => registry_unavailable(&error),
    }
}

fn app_json(app: &App) -> Value {
    json!({
        "appId": app.app_id,
        "name": app.name,
        "createdAt": app.created_at,
    })
}

fn invalid_input(invalid: InvalidInput) -> JsonResponse {
    error_response(StatusCode::BAD_REQUEST, invalid.0)
}

fn registry_unavailable(error: &anyhow::Error) -> JsonResponse {
    tracing::warn!(error = format!("{error:#}"), "registry access failed");
    error_response(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
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

fn empty_response(status: StatusCode) -> JsonResponse {
    let mut response = Response::new(Full::default());
    *response.status_mut() = status;
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

    async fn send(
        console: &TestConsole,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}{path}", console.base_url))
            .header(CSRF_HEADER, "1");
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await.unwrap()
    }

    fn row<'a>(registry: &'a Value, app_id: &str) -> Option<&'a Value> {
        registry
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["appId"] == app_id)
    }

    #[tokio::test]
    async fn create_app_registers_an_app_owned_by_the_caller() {
        // Arrange
        let console = start_console(development("dev")).await;

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/app",
            Some(json!({ "name": "  site 2  " })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::CREATED);
        let app_id = response.json::<Value>().await.unwrap()["app"]["appId"]
            .as_str()
            .unwrap()
            .to_owned();
        let registry = console.registry_file.body();
        let stored = row(&registry, &app_id).unwrap();
        assert_eq!(stored["sub"], "dev");
        assert_eq!(stored["name"], "site 2");
        assert_eq!(stored["isRelay"], false);
        assert!(!stored["secret"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_app_refuses_a_second_app_for_the_same_caller() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;
        let before = console.registry_file.body();

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/app",
            Some(json!({ "name": "again" })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(console.registry_file.body(), before);
    }

    #[tokio::test]
    async fn create_app_rejects_an_invalid_name() {
        // Arrange
        let console = start_console(development("dev")).await;

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/app",
            Some(json!({ "name": "" })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"],
            "invalid_name"
        );
    }

    #[tokio::test]
    async fn issue_token_signs_the_requested_scope_with_the_app_secret() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/tokens",
            Some(json!({ "publish": "cam1", "ttlSeconds": 3600 })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.json::<Value>().await.unwrap();
        let token = body["token"].as_str().unwrap();
        assert!(auth_token::verify_signature(token, "s").is_ok());
        let claims = auth_token::decode_unverified_payload(token).unwrap();
        assert_eq!(claims["appId"], "app-1");
        assert_eq!(claims["publish"], "cam1");
        assert!(claims.get("subscribe").is_none());
        assert_eq!(
            claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(),
            3600
        );
        assert_eq!(body["expiresAt"], claims["exp"]);
    }

    #[tokio::test]
    async fn issue_token_rejects_a_request_with_an_unknown_field() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/tokens",
            Some(json!({ "publish": "", "ttlSeconds": 60, "appId": "relay" })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn issue_token_answers_404_for_a_caller_without_an_app() {
        // Arrange
        let console = start_console(development("nobody")).await;

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/tokens",
            Some(json!({ "publish": "", "ttlSeconds": 60 })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rotate_secret_invalidates_tokens_issued_before() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;
        let issued = send(
            &console,
            reqwest::Method::POST,
            "/api/tokens",
            Some(json!({ "publish": "", "ttlSeconds": 60 })),
        )
        .await
        .json::<Value>()
        .await
        .unwrap();

        // Act
        let response = send(&console, reqwest::Method::POST, "/api/app/secret", None).await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.json::<Value>().await.unwrap()["app"]
                .get("secret")
                .is_none()
        );
        let registry = console.registry_file.body();
        let secret = row(&registry, "app-1").unwrap()["secret"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(secret, "s");
        assert!(auth_token::verify_signature(issued["token"].as_str().unwrap(), &secret).is_err());
    }

    #[tokio::test]
    async fn delete_app_removes_only_the_callers_app() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = send(&console, reqwest::Method::DELETE, "/api/app", None).await;

        // Assert
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let registry = console.registry_file.body();
        assert!(row(&registry, "app-1").is_none());
        assert!(row(&registry, "relay").is_some());
    }

    #[tokio::test]
    async fn a_write_without_the_console_header_is_forbidden() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = reqwest::Client::new()
            .delete(format!("{}/api/app", console.base_url))
            .send()
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(row(&console.registry_file.body(), "app-1").is_some());
    }

    #[tokio::test]
    async fn a_write_from_another_origin_is_forbidden() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = reqwest::Client::new()
            .delete(format!("{}/api/app", console.base_url))
            .header(CSRF_HEADER, "1")
            .header("origin", "https://attacker.example")
            .send()
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_write_from_the_consoles_own_origin_is_accepted() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = reqwest::Client::new()
            .delete(format!("{}/api/app", console.base_url))
            .header(CSRF_HEADER, "1")
            .header("origin", &console.base_url)
            .send()
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
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
