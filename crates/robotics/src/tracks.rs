use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use broadcast::Broadcast;
use bytes::Bytes;
use moqt::publish::{GroupBoundary, OutgoingObject};
use msf::{KnownPackaging, Track, TrackRole};
use serde::Serialize;

use crate::record::{Record, encode_object};

/// The event timeline tracks of a robot room
/// (`docs/architecture/robotics/architecture.md`, Tracks). The robot's
/// broadcast publishes `State`, `Control` and `Episode`; each driver's
/// broadcast (an operator or the policy) publishes `Action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrackKind {
    State,
    Control,
    Episode,
    Action,
}

impl TrackKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Control => "control",
            Self::Episode => "episode",
            Self::Action => "action",
        }
    }

    /// The `eventType` (draft-ietf-moq-msf-02 §5.2.6) defining the `data`
    /// of the track's records.
    pub fn event_type(self) -> String {
        format!("io.skyway.research.moqt.robotics.{}.v1", self.name())
    }

    /// The Publisher Priority of the track and the Subscriber Priority a
    /// driver subscribes to it with; lower values go first (draft-14 §7).
    pub fn priority(self) -> u8 {
        match self {
            Self::Action | Self::Control => 0,
            Self::State => 32,
            Self::Episode => 64,
        }
    }

    /// The track's catalog entry: a live event timeline
    /// (draft-ietf-moq-msf-02 §8.3) of JSON records.
    pub fn catalog_track(self, namespace: Option<String>) -> Track {
        let mut track = Track::new(self.name(), KnownPackaging::EventTimeline, true);
        track.namespace = namespace;
        track.role = Some(TrackRole::Other(self.name().to_string()));
        track.event_type = Some(self.event_type());
        track.label = Some(self.label().to_string());
        track.mime_type = Some("application/json".to_string());
        track
    }

    fn label(self) -> &'static str {
        match self {
            Self::State => "Robot state",
            Self::Control => "Robot control",
            Self::Episode => "Episode",
            Self::Action => "Action",
        }
    }
}

/// Sends `data` as one record of `kind` on `broadcast`, stamped now.
pub(crate) async fn write_record<D: Serialize>(
    broadcast: &Broadcast,
    kind: TrackKind,
    group: GroupBoundary,
    data: D,
) -> Result<()> {
    let payload = encode_object(&Record { t: now_ms(), data })?;
    broadcast
        .send_object(
            kind.name(),
            OutgoingObject::plain(group, Bytes::from(payload)),
        )
        .await?;
    Ok(())
}

/// This host's wall clock in milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const KINDS: [TrackKind; 4] = [
        TrackKind::State,
        TrackKind::Control,
        TrackKind::Episode,
        TrackKind::Action,
    ];

    #[test]
    fn event_types_are_versioned_under_the_relay_domain() {
        // Act
        let event_types: Vec<String> = KINDS.iter().map(|kind| kind.event_type()).collect();

        // Assert
        assert_eq!(
            event_types,
            [
                "io.skyway.research.moqt.robotics.state.v1",
                "io.skyway.research.moqt.robotics.control.v1",
                "io.skyway.research.moqt.robotics.episode.v1",
                "io.skyway.research.moqt.robotics.action.v1",
            ]
        );
    }

    #[test]
    fn control_tracks_go_before_state_and_state_before_episode() {
        // Act
        let priorities: Vec<u8> = KINDS.iter().map(|kind| kind.priority()).collect();

        // Assert: the design's Priority and delivery table
        assert_eq!(priorities, [32, 0, 64, 0]);
    }

    #[test]
    fn catalog_track_describes_a_json_event_timeline() {
        // Act
        let track = TrackKind::State.catalog_track(Some("rooms/lab/robot".to_string()));

        // Assert
        assert_eq!(
            serde_json::to_value(&track).unwrap(),
            json!({
                "namespace": "rooms/lab/robot",
                "name": "state",
                "packaging": "eventtimeline",
                "eventType": "io.skyway.research.moqt.robotics.state.v1",
                "role": "state",
                "isLive": true,
                "label": "Robot state",
                "mimeType": "application/json",
            })
        );
    }
}
