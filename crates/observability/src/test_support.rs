use http_body_util::{BodyExt, Full};
use hyper::{
    Request, Response, StatusCode,
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use relay_stats::{ProcessStats, RelaySnapshot};
use tokio::{net::TcpListener, sync::mpsc};

pub(crate) fn snapshot(timestamp_ms: u64) -> RelaySnapshot {
    RelaySnapshot {
        relay_id: "relay-a".to_string(),
        timestamp_ms,
        process: ProcessStats::default(),
        sessions: vec![],
        tracks: vec![],
        subscriptions: vec![],
    }
}

/// Answers 200 to every request and sends its URI and body to the receiver.
pub(crate) async fn fake_http_server() -> (String, mpsc::UnboundedReceiver<(String, String)>) {
    fake_http_server_answering(StatusCode::OK, "").await
}

/// Answers every request with `status` and `body` and sends its URI and body to the receiver.
pub(crate) async fn fake_http_server_answering(
    status: StatusCode,
    body: &'static str,
) -> (String, mpsc::UnboundedReceiver<(String, String)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (request_sender, request_receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let request_sender = request_sender.clone();
            let service = service_fn(move |request: Request<Incoming>| {
                let request_sender = request_sender.clone();
                async move {
                    let uri = request.uri().to_string();
                    let received = request.into_body().collect().await?.to_bytes();
                    let received = String::from_utf8(received.to_vec()).unwrap();
                    request_sender.send((uri, received)).unwrap();
                    let mut response =
                        Response::new(Full::new(Bytes::from_static(body.as_bytes())));
                    *response.status_mut() = status;
                    Ok::<_, hyper::Error>(response)
                }
            });
            tokio::spawn(http1::Builder::new().serve_connection(TokioIo::new(stream), service));
        }
    });
    (url, request_receiver)
}
