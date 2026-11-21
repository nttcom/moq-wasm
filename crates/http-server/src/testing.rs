use std::sync::{Arc, Mutex};

use http_body_util::{BodyExt, Full};
use hyper::{
    HeaderMap, Method, Response, StatusCode,
    body::{Bytes, Incoming},
};
use tokio::net::TcpListener;

use crate::HttpServer;

#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: Method,
    pub uri: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// An in-process HTTP/1 server bound to `127.0.0.1:0` that records every
/// incoming request. Dropping it stops accepting new connections.
pub struct StubServer {
    url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    _server: HttpServer,
}

impl StubServer {
    pub async fn run(status: StatusCode, body: impl Into<Bytes>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback test listener");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let body = body.into();
        let server = HttpServer::run(listener, move |request: hyper::Request<Incoming>, _peer| {
            let recorded = Arc::clone(&recorded);
            let body = body.clone();
            async move {
                let (parts, incoming) = request.into_parts();
                let incoming = incoming
                    .collect()
                    .await
                    .map(|collected| collected.to_bytes())
                    .unwrap_or_default();
                recorded.lock().unwrap().push(RecordedRequest {
                    method: parts.method,
                    uri: parts.uri.to_string(),
                    headers: parts.headers,
                    body: incoming,
                });
                let mut response = Response::new(Full::new(body));
                *response.status_mut() = status;
                response
            }
        });
        Self {
            url,
            requests,
            _server: server,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };

    use super::*;

    #[tokio::test]
    async fn stub_server_answers_and_records_requests() {
        // Arrange
        let stub = StubServer::run(StatusCode::CREATED, r#"{"ok":true}"#).await;
        let address = stub.url().strip_prefix("http://").unwrap();
        let mut stream = TcpStream::connect(address).await.unwrap();

        // Act
        stream
            .write_all(
                b"POST /items?a=1 HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
            )
            .await
            .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();

        // Assert
        assert!(answer.starts_with("HTTP/1.1 201 Created\r\n"), "{answer}");
        assert!(answer.ends_with(r#"{"ok":true}"#), "{answer}");
        let recorded = stub.requests();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].method, Method::POST);
        assert_eq!(recorded[0].uri, "/items?a=1");
        assert_eq!(recorded[0].body, Bytes::from_static(b"hello"));
    }
}
