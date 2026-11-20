use anyhow::Result;
use broadcast::Broadcast;
use bytes::Bytes;
use serde::Serialize;

pub(crate) const STATE_TRACK_NAME: &str = "call";

/// A call's progress, one JSON object per change on the `call` track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallState {
    Dialing,
    Ringing,
    Answered,
    Ended(EndReason),
}

/// Why a call ended, as the `reason` of its last state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EndReason {
    /// The trunk's final response to the INVITE, or the status the gateway
    /// would have refused an unusable answer with.
    Status(u16),
    /// BYE from the trunk, or the peer's `hangup` or end of its broadcast.
    Hangup,
    /// Not answered within the ringing timeout, or no audio from the trunk.
    Timeout,
    /// The maximum call duration.
    Limit,
    /// The gateway stopped: SIGTERM or a failure of its own.
    Gateway,
}

#[derive(Serialize)]
struct StateObject<'a> {
    state: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl CallState {
    fn to_json(&self) -> Vec<u8> {
        let (state, reason) = match self {
            Self::Dialing => ("dialing", None),
            Self::Ringing => ("ringing", None),
            Self::Answered => ("answered", None),
            Self::Ended(reason) => ("ended", Some(reason.label())),
        };
        serde_json::to_vec(&StateObject { state, reason }).expect("the state serializes")
    }
}

impl EndReason {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Status(status) => status.to_string(),
            Self::Hangup => "hangup".to_string(),
            Self::Timeout => "timeout".to_string(),
            Self::Limit => "limit".to_string(),
            Self::Gateway => "gateway".to_string(),
        }
    }
}

/// The `call` track of the phone's broadcast. Each state is a group of its
/// own, so a late subscriber joining the current group reads the current
/// state, and closing the session after `ended` still delivers it.
pub(crate) struct StateTrack {
    broadcast: Broadcast,
}

impl StateTrack {
    /// Announces the broadcast with the track. Fails when the relay session
    /// cannot be opened or refuses the namespace.
    pub(crate) async fn open(broadcast: Broadcast) -> Result<Self> {
        broadcast
            .declare_tracks(&[STATE_TRACK_NAME.to_string()])
            .await?;
        Ok(Self { broadcast })
    }

    pub(crate) async fn publish(&self, state: &CallState) -> Result<()> {
        self.broadcast
            .send_group(STATE_TRACK_NAME, Bytes::from(state.to_json()))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_is_one_json_object_with_the_reason_only_when_ended() {
        // Act / Assert
        assert_eq!(CallState::Ringing.to_json(), br#"{"state":"ringing"}"#);
        assert_eq!(
            CallState::Ended(EndReason::Status(486)).to_json(),
            br#"{"state":"ended","reason":"486"}"#
        );
        assert_eq!(
            CallState::Ended(EndReason::Hangup).to_json(),
            br#"{"state":"ended","reason":"hangup"}"#
        );
    }
}
