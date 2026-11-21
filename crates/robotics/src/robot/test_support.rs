use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use broadcast::{Broadcast, RelayConnection};
use bytes::Bytes;
use moqt::{
    CertificateVerification, ClientConfig, Endpoint, NATIVE,
    client::{self, GroupStartOptions, PublishTrackOptions, Subscription, TrackWriter, WatchEvent},
    object::ExtensionHeaders,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
};
use serde::{Serialize, de::DeserializeOwned};
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinHandle,
};

use crate::{
    arbiter::ArbiterConfig,
    record::{Record, decode_object, encode_object},
    tracks::TrackKind,
};

use super::{Command, Robot};
use crate::tracks::now_ms;

pub(super) const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// `future`, or a panic once `TEST_TIMEOUT` has passed.
pub(super) async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(TEST_TIMEOUT, future)
        .await
        .expect("timed out")
}

/// The room as the robot's relay session sees it: one peer session that
/// publishes the candidates' namespaces and tracks, as a relay forwards
/// them, and accepts every PUBLISH of the robot's tracks.
pub(super) struct TestRoom {
    pub(super) room: String,
    pub(super) peer: Arc<client::Session>,
    robot_session: Option<Arc<client::Session>>,
    robot_tracks: Mutex<RobotTracks>,
    _accept_task: JoinHandle<()>,
}

/// The subscriptions to the robot's tracks, by track name, as they arrive.
struct RobotTracks {
    receiver: mpsc::UnboundedReceiver<(String, Subscription)>,
    received: HashMap<String, Subscription>,
}

impl TestRoom {
    pub(super) async fn spawn(name: &str, room: &str) -> Self {
        let (robot_session, peer) = connected_sessions(name).await;
        let watch = within(peer.watch_tracks(&elements(&format!("{room}/robot"))))
            .await
            .unwrap();
        let (track_sender, receiver) = mpsc::unbounded_channel();
        let accept_task = tokio::spawn(async move {
            while let Some(event) = watch.next().await {
                let WatchEvent::Track(request) = event else {
                    continue;
                };
                let track = request.track().to_string();
                let subscription = request
                    .accept_current_group(GroupStartOptions::default())
                    .await
                    .unwrap();
                let _ = track_sender.send((track, subscription));
            }
        });
        Self {
            room: room.to_string(),
            peer,
            robot_session: Some(robot_session),
            robot_tracks: Mutex::new(RobotTracks {
                receiver,
                received: HashMap::new(),
            }),
            _accept_task: accept_task,
        }
    }

    /// The subscription to the robot's `kind` track; each is taken once.
    pub(super) async fn robot_track(&self, kind: TrackKind) -> Subscription {
        let mut tracks = self.robot_tracks.lock().await;
        loop {
            if let Some(subscription) = tracks.received.remove(kind.name()) {
                return subscription;
            }
            let (track, subscription) = within(tracks.receiver.recv())
                .await
                .expect("the room's session ended");
            tracks.received.insert(track, subscription);
        }
    }
}

/// A client and a server session over raw QUIC on 127.0.0.1, as (robot,
/// peer).
async fn connected_sessions(name: &str) -> (Arc<client::Session>, Arc<client::Session>) {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("robotics-test-{name}-{}", std::process::id()));
    let CertificateFiles {
        cert_path,
        key_path,
    } = write_self_signed_cert(&dir, &["localhost", "127.0.0.1"]).unwrap();
    let server = Endpoint::<NATIVE>::create_server(&ServerConfig {
        port: 0,
        cert_path,
        key_path,
        keep_alive_interval_sec: 5,
        versions: moqt::DEFAULT_VERSIONS.to_vec(),
    })
    .unwrap();
    let url = format!("moqt://127.0.0.1:{}", server.local_port().unwrap());
    let client = Endpoint::<NATIVE>::create_client(&ClientConfig {
        certificate_verification: CertificateVerification::Disabled,
        ..ClientConfig::default()
    })
    .unwrap();
    let (robot, peer) = within(async {
        tokio::join!(
            async { client.connect(&url).await.unwrap().await.unwrap() },
            async {
                let handshake = server.accept().await.unwrap().await.unwrap();
                handshake.accept(&tracing::Span::current()).await.unwrap()
            }
        )
    })
    .await;
    (
        Arc::new(client::Session::new(robot)),
        Arc::new(client::Session::new(peer)),
    )
}

pub(super) fn elements(path: &str) -> Vec<String> {
    path.split('/').map(str::to_string).collect()
}

/// A broadcast published on `session`, as the SDKs publish one.
pub(super) fn broadcast_on(session: &client::Session, namespace: &str) -> Broadcast {
    let connection = RelayConnection::with_client_session(session);
    session.add_handler(Arc::new(connection.clone()));
    Broadcast::new(connection, elements(namespace))
}

/// A robot in `room` and the session it runs on.
pub(super) struct TestRobot {
    pub(super) robot: Robot,
    pub(super) session: Arc<client::Session>,
}

impl TestRobot {
    /// The robot of `room`, on the session its peer is connected to.
    pub(super) async fn run(room: &mut TestRoom, config: ArbiterConfig) -> Self {
        let session = room.robot_session.take().expect("one robot per room");
        let broadcast = broadcast_on(&session, &format!("{}/robot", room.room));
        let robot = within(Robot::run(
            session.clone(),
            &broadcast,
            elements(&room.room),
            config,
        ))
        .await
        .unwrap();
        Self { robot, session }
    }

    pub(super) async fn next_command(&mut self) -> Command {
        within(self.robot.next_command())
            .await
            .expect("the robot stopped")
    }
}

/// A candidate publishing `action` in `namespace` on the
/// room's peer session, as a relay forwards an operator page's.
pub(super) struct TestDriver {
    peer: Arc<client::Session>,
    namespace: Vec<String>,
    announcement: client::Announcement,
    tracks: HashMap<TrackKind, TrackWriter>,
    action_seq: u64,
}

impl TestDriver {
    pub(super) async fn join(room: &TestRoom, namespace: &str) -> Self {
        let mut driver = Self::announce(room, namespace).await;
        driver.publish_tracks().await;
        driver
    }

    /// Publishes the namespace alone, without its tracks.
    pub(super) async fn announce(room: &TestRoom, namespace: &str) -> Self {
        let namespace = elements(namespace);
        let announcement = within(room.peer.announce(&namespace)).await.unwrap();
        Self {
            peer: room.peer.clone(),
            namespace,
            announcement,
            tracks: HashMap::new(),
            action_seq: 0,
        }
    }

    pub(super) async fn publish_tracks(&mut self) {
        for kind in [TrackKind::Action] {
            let options = PublishTrackOptions {
                publish: true,
                priority: kind.priority(),
                ..PublishTrackOptions::default()
            };
            let writer = within(
                self.peer
                    .publish_track(&self.namespace, kind.name(), options),
            )
            .await
            .unwrap();
            self.tracks.insert(kind, writer);
        }
    }

    /// PUBLISH_NAMESPACE_DONE, as the relay sends once the operator's session
    /// ends.
    pub(super) async fn leave(&self) {
        within(self.announcement.close()).await.unwrap();
    }

    pub(super) async fn send(&self, kind: TrackKind, data: impl Serialize) {
        let payload = encode_object(&Record { t: now_ms(), data }).unwrap();
        self.tracks[&kind]
            .write(Bytes::from(payload), true, ExtensionHeaders::default())
            .await
            .unwrap();
    }

    /// Sends a chunk of one entry answering an observation made now.
    pub(super) async fn send_fresh_action(&mut self, values: Vec<f64>) {
        self.action_seq += 1;
        let action = crate::record::Action {
            seq: self.action_seq,
            basis: now_ms(),
            dt_ms: 20,
            actions: vec![values],
        };
        self.send(TrackKind::Action, action).await;
    }
}

/// The data of the next record on `subscription`.
pub(super) async fn next_record<D: DeserializeOwned>(subscription: &Subscription) -> D {
    let object = within(subscription.next())
        .await
        .unwrap()
        .expect("the track ended");
    decode_object::<D>(&object.payload).unwrap().remove(0).data
}
