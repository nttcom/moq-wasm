use anyhow::{Context, Result};
use moqt::client::{self, GroupStartOptions};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::media::dtmf::event_code;

/// The page's track of commands: one JSON object per command, each a group of
/// its own (`PhoneCall` in the browser SDK).
pub(crate) const CONTROL_TRACK_NAME: &str = "control";

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum Command {
    /// Also sent when the peer ends its broadcast.
    Hangup,
    /// One DTMF digit for the phone: `0`-`9`, `*`, `#` or `A`-`D`.
    Dtmf { digit: char },
    /// An event the gateway does not act on.
    #[serde(other)]
    Other,
}

/// The command an object of the `control` track carries; `None` for one the
/// gateway does not act on.
fn parse_command(payload: &[u8]) -> Option<Command> {
    match serde_json::from_slice(payload).ok()? {
        Command::Other => None,
        Command::Dtmf { digit } if event_code(digit).is_none() => None,
        command => Some(command),
    }
}

/// Reads the peer's `control` track from its current group on, until the
/// track or the session ends. A `hangup` the page wrote before the gateway
/// subscribed still ends the call; a digit already in that group is stale and
/// skipped.
pub(crate) async fn read_commands(
    session: &client::Session,
    namespace: &[String],
    command_sender: &mpsc::Sender<Command>,
) -> Result<()> {
    let subscription = session
        .subscribe_current_group(namespace, CONTROL_TRACK_NAME, GroupStartOptions::default())
        .await
        .context("subscribe to the peer's control track")?;
    let joined_up_to = subscription
        .largest()
        .map(|largest| (largest.group_id, largest.object_id));
    while let Some(object) = subscription.next().await? {
        let Some(command) = parse_command(&object.payload) else {
            continue;
        };
        let joined =
            joined_up_to.is_some_and(|largest| (object.group_id, object.object_id) <= largest);
        if joined && command != Command::Hangup {
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

    #[test]
    fn a_command_is_a_hangup_or_one_dtmf_digit() {
        // Act / Assert
        assert_eq!(
            parse_command(br#"{"event":"hangup"}"#),
            Some(Command::Hangup)
        );
        assert_eq!(
            parse_command(br##"{"event":"dtmf","digit":"#"}"##),
            Some(Command::Dtmf { digit: '#' })
        );
        assert_eq!(parse_command(br#"{"event":"dtmf","digit":"12"}"#), None);
        assert_eq!(parse_command(br#"{"event":"dtmf","digit":"E"}"#), None);
        assert_eq!(parse_command(br#"{"event":"mute"}"#), None);
        assert_eq!(parse_command(br#"{"state":"hangup"}"#), None);
        assert_eq!(parse_command(b"hangup"), None);
    }
}
