use broadcast::RenditionTrack;
use mediapack::MediaEvent;
use moqt::executor::{self, JoinHandle};
use tokio::sync::mpsc;

/// Pushes a rendition's events into its track in the order they come, until
/// the track refuses one or the task is dropped.
pub(super) struct RenditionTask {
    event_sender: mpsc::UnboundedSender<MediaEvent>,
    join_handle: JoinHandle,
}

impl RenditionTask {
    pub(super) fn run(mut track: RenditionTrack, track_name: String) -> Self {
        let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
        let _runtime = pyo3_async_runtimes::tokio::get_runtime().enter();
        let join_handle = executor::spawn("python-media-rendition", async move {
            while let Some(event) = event_receiver.recv().await {
                if let Err(error) = track.push(event).await {
                    // As the source's publish task does, the first failure
                    // stops the rendition.
                    tracing::error!(error = %format!("{error:#}"), track_name, "rendition publishing stopped");
                    return;
                }
            }
        });
        Self {
            event_sender,
            join_handle,
        }
    }

    /// Fails once the track refused an earlier event, which ended the task.
    pub(super) fn push(&self, event: MediaEvent) -> anyhow::Result<()> {
        self.event_sender
            .send(event)
            .map_err(|_| anyhow::anyhow!("rendition publishing stopped"))
    }
}

impl Drop for RenditionTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
