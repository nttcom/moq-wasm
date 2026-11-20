use mediapack::MediaEvent;
use moqt::executor::{self, JoinHandle};
use tokio::sync::{mpsc, oneshot};

use crate::{broadcast::Broadcast, rendition_track::RenditionTrack};

pub(super) enum RenditionCommand {
    Event(MediaEvent),
    /// `Broadcast::remove_track` for the rendition's track, taken after the
    /// events sent before it.
    Remove(oneshot::Sender<anyhow::Result<()>>),
}

/// Pushes a rendition's events into its track in the order they come, until
/// the track refuses one or the task is dropped.
pub(super) struct RenditionTask {
    command_sender: mpsc::UnboundedSender<RenditionCommand>,
    join_handle: JoinHandle,
}

impl RenditionTask {
    pub(super) fn run(mut track: RenditionTrack, broadcast: Broadcast, track_name: String) -> Self {
        let (command_sender, mut command_receiver) = mpsc::unbounded_channel();
        let track_namespace = broadcast.namespace().join("/");
        let join_handle = executor::spawn("media-source-rendition", async move {
            while let Some(command) = command_receiver.recv().await {
                match command {
                    RenditionCommand::Event(event) => {
                        if let Err(error) = track.push(event).await {
                            tracing::warn!(error = %format!("{error:#}"), track_namespace, track_name, "rendition publishing stopped");
                            return;
                        }
                    }
                    RenditionCommand::Remove(done_sender) => {
                        let _ = done_sender.send(broadcast.remove_track(&track_name).await);
                    }
                }
            }
        });
        Self {
            command_sender,
            join_handle,
        }
    }

    pub(super) fn command_sender(&self) -> mpsc::UnboundedSender<RenditionCommand> {
        self.command_sender.clone()
    }

    /// Fails once the track refused an earlier event, which ended the task.
    pub(super) fn push(&self, event: MediaEvent) -> anyhow::Result<()> {
        self.command_sender
            .send(RenditionCommand::Event(event))
            .map_err(|_| anyhow::anyhow!("rendition publishing stopped"))
    }
}

impl Drop for RenditionTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
