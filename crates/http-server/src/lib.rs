use std::{convert::Infallible, error::Error, future::Future, net::SocketAddr, sync::Arc};

use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    HeaderMap, Request, Response, StatusCode,
    body::{Body, Bytes, Incoming},
    header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
        AUTHORIZATION, CONTENT_TYPE, HeaderValue,
    },
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use tokio::{net::TcpListener, task::JoinHandle};

/// Accepts connections on a listener and serves each over HTTP/1 on its own
/// task. Dropping it stops accepting; connections already open run to their end.
pub struct HttpServer {
    accept_handle: JoinHandle<()>,
}

impl HttpServer {
    /// Calls `handler` with each request and the address of the peer that sent it.
    pub fn run<H, F, B>(listener: TcpListener, handler: H) -> Self
    where
        H: Fn(Request<Incoming>, SocketAddr) -> F + Send + Sync + 'static,
        F: Future<Output = Response<B>> + Send + 'static,
        B: Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn Error + Send + Sync>>,
    {
        let handler = Arc::new(handler);
        let accept_handle = tokio::spawn(async move {
            loop {
                let (stream, peer) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        tracing::warn!(%error, "accept failed");
                        continue;
                    }
                };
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let response = handler(request, peer);
                        async move { Ok::<_, Infallible>(response.await) }
                    });
                    if let Err(error) = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                    {
                        tracing::debug!(%peer, %error, "connection closed with error");
                    }
                });
            }
        });
        Self { accept_handle }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.accept_handle.abort();
    }
}

/// A body that does not serialize is sent as `null`.
pub fn json_response(status: StatusCode, body: &impl Serialize) -> Response<Full<Bytes>> {
    let body = serde_json::to_vec(body).unwrap_or_else(|_| b"null".to_vec());
    let mut response = Response::new(Full::from(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

/// `{"error": error}` as JSON.
pub fn error_response(status: StatusCode, error: &str) -> Response<Full<Bytes>> {
    json_response(status, &json!({ "error": error }))
}

pub fn empty_response(status: StatusCode) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::default());
    *response.status_mut() = status;
    response
}

/// `methods` and `headers` are comma-separated lists; panics if either is not
/// a valid header value.
pub fn allow_any_origin<B>(
    mut response: Response<B>,
    methods: &'static str,
    headers: &'static str,
) -> Response<B> {
    let response_headers = response.headers_mut();
    response_headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response_headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static(methods),
    );
    response_headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(headers),
    );
    response
}

/// `None` when the `Authorization` header is absent, is not UTF-8, is not
/// `Bearer`, or has an empty token.
pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

pub const MAX_JSON_BODY_BYTES: usize = 16 * 1024;

/// `None` when the body is longer than `MAX_JSON_BODY_BYTES`, fails to read,
/// or is not JSON of `T`.
pub async fn read_json<T, B>(body: B) -> Option<T>
where
    T: DeserializeOwned,
    B: Body,
    B::Error: Into<Box<dyn Error + Send + Sync>>,
{
    let bytes = Limited::new(body, MAX_JSON_BODY_BYTES)
        .collect()
        .await
        .ok()?
        .to_bytes();
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use http_body_util::BodyExt;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };

    use super::*;

    async fn body_of(response: Response<Full<Bytes>>) -> Bytes {
        response.into_body().collect().await.unwrap().to_bytes()
    }

    #[tokio::test]
    async fn a_request_is_answered_by_the_handler_with_its_peer() {
        // Arrange
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _server = HttpServer::run(listener, |request, peer: SocketAddr| async move {
            json_response(
                StatusCode::OK,
                &json!({ "path": request.uri().path(), "peer": peer.ip().to_string() }),
            )
        });
        let mut stream = TcpStream::connect(address).await.unwrap();

        // Act
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();

        // Assert
        assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
        assert!(
            answer.contains("content-type: application/json\r\n"),
            "{answer}"
        );
        assert!(
            answer.ends_with(r#"{"path":"/healthz","peer":"127.0.0.1"}"#),
            "{answer}"
        );
    }

    #[tokio::test]
    async fn an_error_response_carries_the_error_code() {
        // Act
        let response = error_response(StatusCode::NOT_FOUND, "not_found");

        // Assert
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(body_of(response).await, r#"{"error":"not_found"}"#);
    }

    #[tokio::test]
    async fn an_empty_response_has_its_status_and_no_body() {
        // Act
        let response = empty_response(StatusCode::NO_CONTENT);

        // Assert
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(response.headers().is_empty());
        assert_eq!(body_of(response).await, "");
    }

    #[test]
    fn any_origin_is_allowed_the_given_methods_and_headers() {
        // Act
        let response = allow_any_origin(
            empty_response(StatusCode::NO_CONTENT),
            "GET, POST",
            "authorization",
        );

        // Assert
        let headers = response.headers();
        assert_eq!(headers[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(headers[ACCESS_CONTROL_ALLOW_METHODS], "GET, POST");
        assert_eq!(headers[ACCESS_CONTROL_ALLOW_HEADERS], "authorization");
    }

    fn authorization(value: &str) -> HeaderMap {
        HeaderMap::from_iter([(AUTHORIZATION, HeaderValue::from_str(value).unwrap())])
    }

    #[test]
    fn a_bearer_token_is_read_from_the_authorization_header() {
        // Act / Assert
        assert_eq!(bearer_token(&authorization("Bearer jwt")), Some("jwt"));
    }

    #[test]
    fn an_absent_empty_or_other_scheme_authorization_has_no_bearer_token() {
        // Arrange
        let headers = [
            HeaderMap::new(),
            authorization("Bearer "),
            authorization("Basic dXNlcg=="),
        ];

        // Act
        let tokens: Vec<_> = headers.iter().map(bearer_token).collect();

        // Assert
        assert_eq!(tokens, [None, None, None]);
    }

    #[tokio::test]
    async fn a_json_body_is_read() {
        // Act
        let read: Option<BTreeMap<String, u32>> =
            read_json(Full::<Bytes>::from(r#"{"a":1}"#)).await;

        // Assert
        assert_eq!(read, Some(BTreeMap::from([("a".to_string(), 1)])));
    }

    #[tokio::test]
    async fn a_body_that_is_not_json_of_the_type_is_not_read() {
        // Act
        let read: Option<BTreeMap<String, u32>> =
            read_json(Full::<Bytes>::from(r#"{"a":"b"}"#)).await;

        // Assert
        assert_eq!(read, None);
    }

    #[tokio::test]
    async fn a_body_over_the_limit_is_not_read() {
        // Arrange: valid JSON one byte over the limit
        let body = format!("\"{}\"", "a".repeat(MAX_JSON_BODY_BYTES - 1));

        // Act
        let read: Option<String> = read_json(Full::<Bytes>::from(body)).await;

        // Assert
        assert_eq!(read, None);
    }

    #[tokio::test]
    async fn a_body_that_does_not_serialize_is_sent_as_null() {
        // Arrange: JSON object keys must be strings
        let body = BTreeMap::from([((1, 2), "pair key")]);

        // Act
        let response = json_response(StatusCode::OK, &body);

        // Assert
        assert_eq!(body_of(response).await, "null");
    }
}
