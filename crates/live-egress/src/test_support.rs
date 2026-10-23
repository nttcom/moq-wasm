use moqt::{
    DUAL, Endpoint, SessionEvent,
    publish::Origin,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
};
use tokio::sync::mpsc;

/// A publisher that serves the tracks registered with `origin` to the one
/// session that connects to `url`, as a relay would.
pub(crate) struct FakePublisher {
    pub(crate) url: String,
    pub(crate) origin: Origin,
    /// The track name of each SUBSCRIBE, sent once `origin` serves it.
    pub(crate) served: mpsc::UnboundedReceiver<String>,
}

impl FakePublisher {
    pub(crate) fn spawn() -> Self {
        let CertificateFiles {
            cert_path,
            key_path,
        } = write_self_signed_cert(&cert_dir(), &["localhost", "127.0.0.1"]).unwrap();
        let endpoint = Endpoint::<DUAL>::create_server(&ServerConfig {
            port: 0,
            cert_path,
            key_path,
            keep_alive_interval_sec: 5,
            versions: moqt::DEFAULT_VERSIONS.to_vec(),
        })
        .unwrap();
        let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
        let origin = Origin::default();
        let (served_sender, served) = mpsc::unbounded_channel();
        let serving = origin.clone();
        tokio::spawn(async move {
            let handshake = endpoint.accept().await.unwrap().await.unwrap();
            let session = handshake.accept().await.unwrap();
            while let Ok(event) = session.receive_event().await {
                let subscribed = match &event {
                    SessionEvent::Subscribe(handler) => Some(handler.track_name.clone()),
                    _ => None,
                };
                serving.handle(event).await;
                if let Some(track_name) = subscribed {
                    let _ = served_sender.send(track_name);
                }
            }
        });
        Self {
            url,
            origin,
            served,
        }
    }
}

fn cert_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("live-egress-test-{}", std::process::id()))
}
