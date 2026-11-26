use std::{path::PathBuf, sync::Arc, time::Duration};

use bytes::Bytes;
use mediapack::{MediaEvent, Timestamp, VideoConfig, VideoSample, mp4::Demuxer, vp8::Vp8Config};

use moqt::{
    CertificateVerification, ClientConfig, Endpoint, NATIVE, Session, SessionEvent,
    publish::Origin,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
};
use tokio::sync::mpsc;

pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(5);

mod fake_relay;

pub(crate) use fake_relay::{FakeRelay, REFUSED_TOKEN, token};

pub(crate) fn vp8_config() -> VideoConfig {
    VideoConfig::Vp8(Vp8Config {
        width: 64,
        height: 48,
    })
}

pub(crate) fn video_sample(dts_us: u64, is_keyframe: bool) -> VideoSample {
    VideoSample {
        data: Bytes::from_static(b"frame"),
        is_keyframe,
        pts: Timestamp::from_micros(dts_us),
        dts: Timestamp::from_micros(dts_us),
    }
}

pub(crate) fn keyframe_at(pts_us: u64) -> MediaEvent {
    MediaEvent::Video(VideoSample {
        data: Bytes::from_static(b"nal"),
        is_keyframe: true,
        pts: Timestamp::from_micros(pts_us),
        dts: Timestamp::from_micros(pts_us),
    })
}

/// The largest gap between a time and the one its CMAF fragment decodes at:
/// a tick of the 90 kHz video timescale, rounded up to whole microseconds.
pub(crate) const VIDEO_TICK_US: u64 = 12;

/// The decode time of each sample `fragments` hold, read back through
/// `init_segment`.
pub(crate) fn decode_times(init_segment: &[u8], fragments: &[Bytes]) -> Vec<Timestamp> {
    let mut demuxer = Demuxer::new();
    let mut events = demuxer.push(init_segment).unwrap();
    for fragment in fragments {
        events.extend(demuxer.push(fragment).unwrap());
    }
    events.extend(demuxer.finish().unwrap());
    events
        .into_iter()
        .filter_map(|event| match event {
            MediaEvent::Video(sample) => Some(sample.dts),
            MediaEvent::Audio(sample) => Some(sample.pts),
            _ => None,
        })
        .collect()
}

/// A server config with a self-signed certificate written under a temporary
/// directory named after `name`, so tests running in parallel do not share one.
pub(crate) fn self_signed_server_config(name: &str) -> ServerConfig {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("broadcast-test-{name}-{}", std::process::id()));
    let CertificateFiles {
        cert_path,
        key_path,
    } = write_self_signed_cert(&dir, &["localhost", "127.0.0.1"]).unwrap();
    ServerConfig {
        port: 0,
        cert_path,
        key_path,
        keep_alive_interval_sec: 5,
        // The tests write and read draft-14 messages.
        versions: vec![moqt::Version::Draft14],
    }
}

/// A client session to `url` that trusts any certificate.
pub(crate) async fn connect(url: &str) -> Arc<Session> {
    let endpoint = Endpoint::<NATIVE>::create_client(&ClientConfig {
        certificate_verification: CertificateVerification::Disabled,
        ..ClientConfig::default()
    })
    .unwrap();
    Arc::new(endpoint.connect(url).await.unwrap().await.unwrap())
}

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
        let endpoint =
            Endpoint::<NATIVE>::create_server(&self_signed_server_config("publisher")).unwrap();
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
