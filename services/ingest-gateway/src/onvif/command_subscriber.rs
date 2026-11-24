use std::sync::mpsc::Sender;

use anyhow::{Context, Result, bail};
use moqt::client::{FollowEvent, Session};
use tokio::task::JoinHandle;

use crate::onvif::ptz_worker::Command;

pub(crate) struct CommandTrack {
    pub(crate) namespace: Vec<String>,
    pub(crate) track: String,
}

/// Follows the viewers' command track on the session the media broadcast
/// publishes on, and forwards each command to the PTZ worker. Each viewer
/// that announces the namespace or publishes the track takes over from the
/// one before.
pub(crate) struct CommandSubscriber {
    task: JoinHandle<Result<()>>,
}

impl CommandSubscriber {
    pub(crate) async fn run(
        session: &Session,
        command_track: CommandTrack,
        command_sender: Sender<Command>,
    ) -> Result<Self> {
        let follower = session
            .follow_track(&command_track.namespace, &command_track.track)
            .await
            .context("follow command track")?;
        tracing::info!(
            track_namespace = %command_track.namespace.join("/"),
            track_name = %command_track.track,
            "following the command track"
        );
        let task = tokio::spawn(async move {
            while let Some(event) = follower.next().await {
                if let FollowEvent::Object(object) = event
                    && let Err(err) = handle_command_payload(&object.payload, &command_sender)
                {
                    tracing::warn!("command payload error: {err}");
                }
            }
            bail!("command session ended")
        });
        Ok(Self { task })
    }

    /// Resolves only with an error: the command session ended.
    pub(crate) async fn finished(&mut self) -> Result<()> {
        (&mut self.task).await.context("command session task")?
    }
}

impl Drop for CommandSubscriber {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn handle_command_payload(payload: &[u8], command_sender: &Sender<Command>) -> Result<()> {
    let command: Command = serde_json::from_slice(payload).context("parse command json")?;
    tracing::info!("PTZ command received: {command:?}");
    let _ = command_sender.send(command);
    Ok(())
}
