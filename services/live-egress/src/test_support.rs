use moqt::{
    Endpoint, NATIVE, SessionEvent,
    publish::Origin,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
};
use std::time::Duration;
use tokio::sync::mpsc;

/// A publisher that serves the tracks registered with `origin` to the one
/// session that connects to `url`, as a relay would.
pub(crate) struct FakePublisher {
    pub(crate) url: String,
    pub(crate) origin: Origin,
    /// The track name of each SUBSCRIBE, sent once `origin` serves it.
    served: mpsc::UnboundedReceiver<String>,
}

impl FakePublisher {
    pub(crate) fn spawn() -> Self {
        let CertificateFiles {
            cert_path,
            key_path,
        } = write_self_signed_cert(&cert_dir(), &["localhost", "127.0.0.1"]).unwrap();
        let endpoint = Endpoint::<NATIVE>::create_server(&ServerConfig {
            port: 0,
            cert_path,
            key_path,
            keep_alive_interval_sec: 5,
            // The tests write and read draft-14 messages.
            versions: vec![moqt::Version::Draft14],
        })
        .unwrap();
        let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
        let origin = Origin::default();
        let (served_sender, served) = mpsc::unbounded_channel();
        let serving = origin.clone();
        tokio::spawn(async move {
            let handshake = endpoint.accept().await.unwrap().await.unwrap();
            let session = handshake.accept(&tracing::Span::current()).await.unwrap();
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

    /// The track name of the next SUBSCRIBE `origin` serves.
    ///
    /// # Panics
    /// When none arrives within 5 seconds, as when the subscriber gave up.
    pub(crate) async fn next_served(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(5), self.served.recv())
            .await
            .expect("no track was subscribed")
            .expect("the publisher stopped serving")
    }
}

fn cert_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("live-egress-test-{}", std::process::id()))
}
