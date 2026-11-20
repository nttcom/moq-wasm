use anyhow::{Context, Result};
use moqt::client::{self, GroupStartOptions};
use serde::Deserialize;
use tokio::sync::mpsc;

/// The page's track of commands: one JSON object per command, each a group of
/// its own (`PhoneCall` in the browser SDK).
pub(crate) const CONTROL_TRACK_NAME: &str = "control";

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum Command {
    /// Also sent when the peer ends its broadcast.
    Hangup,
    /// An event the gateway does not act on.
    #[serde(other)]
    Other,
}

/// Reads the peer's `control` track from its current group on, until the
/// track or the session ends, so a `hangup` the page wrote before the gateway
/// subscribed still ends the call.
pub(crate) async fn read_commands(
    session: &client::Session,
    namespace: &[String],
    command_sender: &mpsc::Sender<Command>,
) -> Result<()> {
    let subscription = session
        .subscribe_current_group(namespace, CONTROL_TRACK_NAME, GroupStartOptions::default())
        .await
        .context("subscribe to the peer's control track")?;
    while let Some(object) = subscription.next().await? {
        let Ok(command) = serde_json::from_slice::<Command>(&object.payload) else {
            continue;
        };
        if command == Command::Other {
            continue;
        }
        if command_sender.send(command).await.is_err() {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(payload: &[u8]) -> Option<Command> {
        serde_json::from_slice(payload).ok()
    }

    #[test]
    fn an_object_names_its_command_in_event() {
        // Act / Assert
        assert_eq!(parse(br#"{"event":"hangup"}"#), Some(Command::Hangup));
        assert_eq!(parse(br#"{"event":"mute"}"#), Some(Command::Other));
        assert_eq!(parse(br#"{"state":"hangup"}"#), None);
        assert_eq!(parse(b"hangup"), None);
    }
}
