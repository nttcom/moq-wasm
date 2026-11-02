use std::{
    convert::Infallible,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use app_registry::{App, RegistryStore};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{
        CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HOST, HeaderValue, ORIGIN,
        X_CONTENT_TYPE_OPTIONS,
    },
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use reqwest::Url;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{
    apps::{self, Creation},
    identity::{Caller, Identity},
    input::{CreateAppRequest, InvalidInput, IssueTokenRequest},
    static_files::StaticFiles,
};

pub const CSRF_HEADER: &str = "x-console-request";
const MAX_BODY_BYTES: usize = 16 * 1024;
const CONTENT_SECURITY: &str = "default-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

pub struct ConsoleState {
    identity: Identity,
    store: RegistryStore,
    static_files: Option<StaticFiles>,
    relay_url: Option<String>,
}

impl ConsoleState {
    pub fn new(
        identity: Identity,
        store: RegistryStore,
        static_dir: Option<PathBuf>,
        relay_url: Option<String>,
    ) -> Self {
        Self {
            identity,
            store,
            static_files: static_dir.map(StaticFiles::new),
            relay_url,
        }
    }
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

/// The status and error code of a refused request.
struct Refusal(StatusCode, &'static str);

impl Refusal {
    fn response(self) -> JsonResponse {
        error_response(self.0, self.1)
    }
}

async fn handle(
    request: Request<Incoming>,
    state: Arc<ConsoleState>,
) -> Result<JsonResponse, Infallible> {
    let (head, body) = request.into_parts();
    let mut response = if head.method != Method::GET && !is_same_origin_request(&head.headers) {
        error_response(StatusCode::FORBIDDEN, "forbidden")
    } else {
        route(
            head.method.clone(),
            head.uri.path(),
            &head.headers,
            body,
            &state,
        )
        .await
    };
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    Ok(response)
}

async fn route(
    method: Method,
    path: &str,
    headers: &HeaderMap,
    body: Incoming,
    state: &ConsoleState,
) -> JsonResponse {
    let handled = if let Some((app_id, action)) = app_route(path) {
        match (method, action) {
            (Method::POST, Some("tokens")) => issue_token(headers, body, app_id, state).await,
            (Method::POST, Some("secret")) => rotate_secret(headers, app_id, state).await,
            (Method::DELETE, None) => delete_app(headers, app_id, state).await,
            _ => Err(Refusal(StatusCode::NOT_FOUND, "not_found")),
        }
    } else {
        match (method, path) {
            (Method::GET, "/api/me") => me(headers, state).await,
            (Method::GET, "/api/apps") => list_apps(headers, state).await,
            (Method::POST, "/api/apps") => create_app(headers, body, state).await,
            (Method::GET, path) if !path.starts_with("/api/") => {
                return static_file(path, state).await;
            }
            _ => Err(Refusal(StatusCode::NOT_FOUND, "not_found")),
        }
    };
    handled.unwrap_or_else(Refusal::response)
}

fn app_route(path: &str) -> Option<(&str, Option<&str>)> {
    let rest = path.strip_prefix("/api/apps/")?;
    Some(match rest.split_once('/') {
        Some((app_id, action)) => (app_id, Some(action)),
        None => (rest, None),
    })
}

async fn static_file(path: &str, state: &ConsoleState) -> JsonResponse {
    let file = match &state.static_files {
        Some(static_files) => static_files.get(path).await,
        None => None,
    };
    let Some(file) = file else {
        return error_response(StatusCode::NOT_FOUND, "not_found");
    };
    let mut response = Response::new(Full::from(file.bytes));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(file.content_type));
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static(if file.immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        }),
    );
    response
}

async fn me(headers: &HeaderMap, state: &ConsoleState) -> Result<JsonResponse, Refusal> {
    let caller = authenticate(headers, state).await?;
    Ok(json_response(
        StatusCode::OK,
        json!({ "email": caller.email, "relayUrl": state.relay_url }),
    ))
}

async fn list_apps(headers: &HeaderMap, state: &ConsoleState) -> Result<JsonResponse, Refusal> {
    let caller = authenticate(headers, state).await?;
    let snapshot = state.store.load().await.map_err(registry_unavailable)?;
    let apps: Vec<Value> = snapshot
        .apps
        .iter()
        .filter(|app| apps::owned_by(app, &caller.sub))
        .map(app_json)
        .collect();
    Ok(json_response(StatusCode::OK, json!({ "apps": apps })))
}

async fn create_app(
    headers: &HeaderMap,
    body: Incoming,
    state: &ConsoleState,
) -> Result<JsonResponse, Refusal> {
    let caller = authenticate(headers, state).await?;
    let request: CreateAppRequest = read_json(body).await?;
    let name = request.name().map_err(invalid_input)?;
    match apps::create_app(&state.store, &caller.sub, name, unix_now())
        .await
        .map_err(registry_unavailable)?
    {
        Creation::Created(app) => {
            tracing::info!(app_id = %app.app_id, "app created");
            Ok(json_response(
                StatusCode::CREATED,
                app_with_secret_json(&app),
            ))
        }
        Creation::LimitReached => Err(Refusal(StatusCode::BAD_REQUEST, "too_many_apps")),
    }
}

async fn issue_token(
    headers: &HeaderMap,
    body: Incoming,
    app_id: &str,
    state: &ConsoleState,
) -> Result<JsonResponse, Refusal> {
    let caller = authenticate(headers, state).await?;
    let request: IssueTokenRequest = read_json(body).await?;
    let scope = request.scope().map_err(invalid_input)?;
    respond(
        apps::issue_token(&state.store, &caller.sub, app_id, scope, unix_now()).await,
        |issued| {
            tracing::info!(app_id, expires_at = issued.expires_at, "token issued");
            json_response(
                StatusCode::OK,
                json!({ "token": issued.token, "expiresAt": issued.expires_at }),
            )
        },
    )
}

async fn rotate_secret(
    headers: &HeaderMap,
    app_id: &str,
    state: &ConsoleState,
) -> Result<JsonResponse, Refusal> {
    let caller = authenticate(headers, state).await?;
    respond(
        apps::rotate_secret(&state.store, &caller.sub, app_id, unix_now()).await,
        |app| {
            tracing::info!(app_id = %app.app_id, "secret rotated");
            json_response(StatusCode::OK, app_with_secret_json(&app))
        },
    )
}

async fn delete_app(
    headers: &HeaderMap,
    app_id: &str,
    state: &ConsoleState,
) -> Result<JsonResponse, Refusal> {
    let caller = authenticate(headers, state).await?;
    respond(
        apps::delete_app(&state.store, &caller.sub, app_id).await,
        |()| {
            tracing::info!(app_id, "app deleted");
            empty_response(StatusCode::NO_CONTENT)
        },
    )
}

async fn authenticate(headers: &HeaderMap, state: &ConsoleState) -> Result<Caller, Refusal> {
    state
        .identity
        .caller(headers, unix_now())
        .await
        .map_err(|error| {
            tracing::info!(error = format!("{error:#}"), "request not authenticated");
            Refusal(StatusCode::UNAUTHORIZED, "unauthenticated")
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
    let invalid_request = || Refusal(StatusCode::BAD_REQUEST, "invalid_request");
    let bytes = Limited::new(body, MAX_BODY_BYTES)
        .collect()
        .await
        .map_err(|_| invalid_request())?
        .to_bytes();
    serde_json::from_slice(&bytes).map_err(|_| invalid_request())
}

fn respond<T>(
    result: anyhow::Result<Option<T>>,
    done: impl FnOnce(T) -> JsonResponse,
) -> Result<JsonResponse, Refusal> {
    match result.map_err(registry_unavailable)? {
        Some(value) => Ok(done(value)),
        None => Err(Refusal(StatusCode::NOT_FOUND, "no_app")),
    }
}

fn app_with_secret_json(app: &App) -> Value {
    json!({ "app": app_json(app), "secret": app.secret })
}

fn app_json(app: &App) -> Value {
    json!({
        "appId": app.app_id,
        "name": app.name,
        "createdAt": app.created_at,
        "rotatedAt": app.rotated_at,
    })
}

fn invalid_input(invalid: InvalidInput) -> Refusal {
    Refusal(StatusCode::BAD_REQUEST, invalid.0)
}

fn registry_unavailable(error: anyhow::Error) -> Refusal {
    tracing::warn!(error = format!("{error:#}"), "registry access failed");
    Refusal(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
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
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn empty_response(status: StatusCode) -> JsonResponse {
    let mut response = Response::new(Full::default());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
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

    const RELAY_URL: &str = "https://relay.example.com";

    struct TestConsole {
        base_url: String,
        registry_file: TemporaryRegistryFile,
        _server: ConsoleServer,
    }

    async fn start_console(identity: Identity) -> TestConsole {
        start_console_with_static_dir(identity, None).await
    }

    async fn start_console_with_static_dir(
        identity: Identity,
        static_dir: Option<PathBuf>,
    ) -> TestConsole {
        let registry_file = TemporaryRegistryFile::new();
        registry_file.put(&json!([
            { "appId": "app-1", "name": "camera", "secret": "s", "isRelay": false,
              "sub": "accounts.google.com:1", "createdAt": 1760900000 },
            { "appId": "app-2", "name": "other", "secret": "t", "isRelay": false,
              "sub": "accounts.google.com:2", "createdAt": 1760900000 },
            { "appId": "relay", "secret": "r", "isRelay": true },
        ]));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(ConsoleState::new(
            identity,
            registry_file.store(),
            static_dir,
            Some(RELAY_URL.into()),
        ));
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
    async fn create_app_registers_an_app_owned_by_the_caller_and_returns_its_secret_once() {
        // Arrange
        let console = start_console(development("dev")).await;

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/apps",
            Some(json!({ "name": "  site 2  " })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = response.json::<Value>().await.unwrap();
        let app_id = created["app"]["appId"].as_str().unwrap().to_owned();
        let registry = console.registry_file.body();
        let stored = row(&registry, &app_id).unwrap();
        assert_eq!(stored["sub"], "dev");
        assert_eq!(stored["name"], "site 2");
        assert_eq!(stored["isRelay"], false);
        assert!(!stored["secret"].as_str().unwrap().is_empty());
        assert_eq!(created["secret"], stored["secret"]);
        assert!(created["app"].get("secret").is_none());
    }

    #[tokio::test]
    async fn create_app_lets_the_caller_own_several_apps() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/apps",
            Some(json!({ "name": "again" })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::CREATED);
        let registry = console.registry_file.body();
        let owned = registry
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["sub"] == "accounts.google.com:1")
            .count();
        assert_eq!(owned, 2);
    }

    #[tokio::test]
    async fn create_app_refuses_an_app_beyond_the_limit_per_owner() {
        // Arrange
        let console = start_console(development("dev")).await;
        let owned: Vec<Value> = (0..apps::MAX_APPS_PER_OWNER)
            .map(|index| json!({ "appId": format!("app-{index}"), "secret": "s", "isRelay": false, "sub": "dev" }))
            .collect();
        console.registry_file.put(&Value::Array(owned));
        let before = console.registry_file.body();

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/apps",
            Some(json!({ "name": "one more" })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"],
            "too_many_apps"
        );
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
            "/api/apps",
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
            "/api/apps/app-1/tokens",
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
            "/api/apps/app-1/tokens",
            Some(json!({ "publish": "", "ttlSeconds": 60, "appId": "relay" })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn issue_token_answers_404_for_an_app_of_another_caller() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/apps/app-2/tokens",
            Some(json!({ "publish": "", "ttlSeconds": 60 })),
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rotate_secret_returns_the_new_secret_records_when_and_invalidates_tokens_issued_before()
     {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;
        let issued = send(
            &console,
            reqwest::Method::POST,
            "/api/apps/app-1/tokens",
            Some(json!({ "publish": "", "ttlSeconds": 60 })),
        )
        .await
        .json::<Value>()
        .await
        .unwrap();

        // Act
        let response = send(
            &console,
            reqwest::Method::POST,
            "/api/apps/app-1/secret",
            None,
        )
        .await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
        let rotated = response.json::<Value>().await.unwrap();
        let registry = console.registry_file.body();
        let secret = row(&registry, "app-1").unwrap()["secret"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(secret, "s");
        assert_eq!(rotated["secret"], secret.as_str());
        assert!(rotated["app"]["rotatedAt"].as_u64().is_some());
        assert_eq!(
            row(&registry, "app-1").unwrap()["rotatedAt"],
            rotated["app"]["rotatedAt"]
        );
        assert!(auth_token::verify_signature(issued["token"].as_str().unwrap(), &secret).is_err());
    }

    #[tokio::test]
    async fn delete_app_removes_only_the_named_app() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = send(&console, reqwest::Method::DELETE, "/api/apps/app-1", None).await;

        // Assert
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let registry = console.registry_file.body();
        assert!(row(&registry, "app-1").is_none());
        assert!(row(&registry, "app-2").is_some());
        assert!(row(&registry, "relay").is_some());
    }

    #[tokio::test]
    async fn delete_app_answers_404_for_an_app_of_another_caller() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = send(&console, reqwest::Method::DELETE, "/api/apps/app-2", None).await;

        // Assert
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(row(&console.registry_file.body(), "app-2").is_some());
    }

    #[tokio::test]
    async fn a_write_without_the_console_header_is_forbidden() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = reqwest::Client::new()
            .delete(format!("{}/api/apps/app-1", console.base_url))
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
            .delete(format!("{}/api/apps/app-1", console.base_url))
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
            .delete(format!("{}/api/apps/app-1", console.base_url))
            .header(CSRF_HEADER, "1")
            .header("origin", &console.base_url)
            .send()
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    struct StaticDir(PathBuf);

    impl StaticDir {
        fn with_index_and_asset() -> Self {
            let root = std::env::temp_dir().join(format!(
                "console-static-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(root.join("assets")).unwrap();
            std::fs::write(root.join("index.html"), "<!doctype html>").unwrap();
            std::fs::write(root.join("assets/app-1.js"), "export {}").unwrap();
            Self(root)
        }
    }

    impl Drop for StaticDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn get(console: &TestConsole, path: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(format!("{}{path}", console.base_url))
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_root_serves_the_index_without_caching() {
        // Arrange
        let static_dir = StaticDir::with_index_and_asset();
        let console =
            start_console_with_static_dir(development("dev"), Some(static_dir.0.clone())).await;

        // Act
        let response = get(&console, "/").await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        assert_eq!(response.headers()["cache-control"], "no-cache");
        assert!(
            response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.text().await.unwrap(), "<!doctype html>");
    }

    #[tokio::test]
    async fn assets_are_cached_for_a_year() {
        // Arrange
        let static_dir = StaticDir::with_index_and_asset();
        let console =
            start_console_with_static_dir(development("dev"), Some(static_dir.0.clone())).await;

        // Act
        let response = get(&console, "/assets/app-1.js").await;

        // Assert
        assert_eq!(
            response.headers()["content-type"],
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            response.headers()["cache-control"],
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn an_unknown_api_path_is_not_answered_with_a_static_file() {
        // Arrange
        let static_dir = StaticDir::with_index_and_asset();
        let console =
            start_console_with_static_dir(development("dev"), Some(static_dir.0.clone())).await;

        // Act
        let response = get(&console, "/api/unknown").await;

        // Assert
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"],
            "not_found"
        );
    }

    #[tokio::test]
    async fn api_answers_are_never_stored() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = get(&console, "/api/me").await;

        // Assert
        assert_eq!(response.headers()["cache-control"], "no-store");
    }

    #[tokio::test]
    async fn list_apps_returns_only_the_callers_apps_without_their_secrets() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = get(&console, "/api/apps").await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({ "apps": [{ "appId": "app-1", "name": "camera", "createdAt": 1760900000,
                               "rotatedAt": null }] })
        );
    }

    #[tokio::test]
    async fn list_apps_is_empty_for_a_caller_without_apps() {
        // Arrange
        let console = start_console(development("accounts.google.com:3")).await;

        // Act
        let response = get(&console, "/api/apps").await;

        // Assert
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({ "apps": [] })
        );
    }

    #[tokio::test]
    async fn me_has_no_email_for_a_development_identity() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;

        // Act
        let response = get_me(&console, None).await;

        // Assert
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({ "email": null, "relayUrl": RELAY_URL })
        );
    }

    #[tokio::test]
    async fn me_returns_the_email_of_a_valid_iap_assertion() {
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
            response.json::<Value>().await.unwrap()["email"],
            "someone@example.com"
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
    async fn list_apps_answers_503_when_the_registry_cannot_be_read() {
        // Arrange
        let console = start_console(development("accounts.google.com:1")).await;
        console.registry_file.write("not a registry");

        // Act
        let response = get(&console, "/api/apps").await;

        // Assert
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
