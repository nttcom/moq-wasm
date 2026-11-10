use std::{convert::Infallible, error::Error, future::Future, net::SocketAddr, sync::Arc};

use http_body_util::Full;
use hyper::{
    Request, Response, StatusCode,
    body::{Body, Bytes, Incoming},
    header::{CONTENT_TYPE, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde::Serialize;
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
    async fn a_body_that_does_not_serialize_is_sent_as_null() {
        // Arrange: JSON object keys must be strings
        let body = BTreeMap::from([((1, 2), "pair key")]);

        // Act
        let response = json_response(StatusCode::OK, &body);

        // Assert
        assert_eq!(body_of(response).await, "null");
    }
}
