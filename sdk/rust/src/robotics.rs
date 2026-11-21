//! The robot side of a robot room
//! (docs/architecture/robotics/architecture.md).

use anyhow::bail;

use crate::{Session, media::LocalBroadcast};

pub use ::robotics::{ArbiterConfig, Command, Driver, HoldReason, Robot, State, decode_object};

/// Runs the robot of the room `broadcast` belongs to: `broadcast` is
/// `<room>/robot`, announced on `session`. Keep `broadcast` alive while the
/// robot runs: dropping it withdraws the namespace the robot's tracks are
/// under. Fails when `broadcast` is not `<room>/robot`, when its tracks
/// cannot be offered, and when the relay rejects the SUBSCRIBE_NAMESPACE for
/// `<room>`, as it does while `session` watches a prefix overlapping `<room>`.
pub async fn run_robot(
    session: &Session,
    broadcast: &LocalBroadcast,
    config: ArbiterConfig,
) -> anyhow::Result<Robot> {
    let broadcast = broadcast.broadcast();
    let room = match broadcast.namespace().split_last() {
        Some((last, room)) if last == "robot" && !room.is_empty() => room.to_vec(),
        _ => bail!(
            "the robot's broadcast is {}, not <room>/robot",
            broadcast.namespace().join("/")
        ),
    };
    Robot::run(session.client().clone(), broadcast, room, config).await
}

/// Starts driving a robot room from `broadcast`, a driver's announced
/// broadcast (`<room>/operators/<id>` or `<room>/bots/policy`): lists its
/// `action` track in the catalog. Keep `broadcast` alive while sending.
/// Fails when the track cannot be offered.
pub async fn offer_actions(broadcast: &LocalBroadcast) -> anyhow::Result<Driver> {
    Driver::offer(broadcast.broadcast()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{media::create_broadcast, test_support::TestRelay};

    #[tokio::test]
    async fn a_robot_offers_its_tracks_and_watches_its_room() {
        // Arrange
        let mut relay = TestRelay::spawn("robot");
        let session = relay.connect().await;
        let broadcast = create_broadcast(&session, &["anon", "lab", "robot"])
            .announce()
            .await
            .unwrap();
        relay.next_request().await;

        // Act
        let _robot = run_robot(&session, &broadcast, ArbiterConfig::default())
            .await
            .unwrap();
        let mut requests = Vec::new();
        while !requests
            .iter()
            .any(|request: &String| request.starts_with("SUBSCRIBE_NAMESPACE"))
        {
            requests.push(relay.next_request().await);
        }

        // Assert
        for track in ["state", "control", "episode"] {
            let publish = format!("PUBLISH anon/lab/robot/{track}");
            assert!(requests.contains(&publish), "{publish} not in {requests:?}");
        }
        assert_eq!(requests.last().unwrap(), "SUBSCRIBE_NAMESPACE anon/lab");
    }

    #[tokio::test]
    async fn a_driver_publishes_its_action_track_and_sends_on_it() {
        // Arrange
        let mut relay = TestRelay::spawn("driver");
        let session = relay.connect().await;
        let broadcast = create_broadcast(&session, &["anon", "lab", "bots", "policy"])
            .announce()
            .await
            .unwrap();
        relay.next_request().await;

        // Act
        let driver = offer_actions(&broadcast).await.unwrap();
        let sent = driver.send(1_000, 50, vec![vec![0.5, 0.0]]).await;

        // Assert
        let mut requests = Vec::new();
        while !requests
            .iter()
            .any(|request: &String| request.ends_with("/catalog"))
        {
            requests.push(relay.next_request().await);
        }
        assert!(
            requests.contains(&"PUBLISH anon/lab/bots/policy/action".to_string()),
            "{requests:?}"
        );
        assert!(sent.is_ok());
    }

    #[tokio::test]
    async fn a_broadcast_other_than_the_rooms_robot_is_refused() {
        // Arrange
        let relay = TestRelay::spawn("robot-refused");
        let session = relay.connect().await;
        let camera = create_broadcast(&session, &["anon", "lab", "camera"])
            .announce()
            .await
            .unwrap();

        // Act
        let result = run_robot(&session, &camera, ArbiterConfig::default()).await;

        // Assert
        assert!(result.is_err());
    }
}
