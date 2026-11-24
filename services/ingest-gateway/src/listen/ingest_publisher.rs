use anyhow::Result;
use broadcast::{
    Broadcast, BroadcastWriter, MoqtTarget, RelayConnection, SourceTracks, VideoTrackInfo,
};
use mediapack::MediaEvent;

use crate::listen::{
    renditions::RenditionFanout,
    stats_panel::{ConnectionRegistry, Registration},
};

#[derive(Clone)]
pub struct IngestOptions {
    pub moqt: Option<MoqtTarget>,
    pub transcode: bool,
    pub stats: Option<ConnectionRegistry>,
}

impl IngestOptions {
    /// A connection to the relay, shown on the stats panel under `label` for
    /// as long as the registration lives.
    pub fn connect(&self, label: String) -> (RelayConnection, Option<Registration>) {
        let connection = RelayConnection::new(self.moqt.clone());
        let registration = self
            .stats
            .as_ref()
            .map(|registry| registry.register(label, &connection));
        (connection, registration)
    }
}

pub struct IngestPublisher {
    writer: BroadcastWriter,
    transcode: bool,
    renditions: Option<RenditionFanout>,
}

impl IngestPublisher {
    pub fn new(broadcast: Broadcast, transcode: bool) -> Self {
        Self {
            writer: BroadcastWriter::run(broadcast, SourceTracks::default()),
            transcode,
            renditions: None,
        }
    }

    pub fn push(&mut self, event: MediaEvent) -> Result<()> {
        let rendition_sample = match &event {
            MediaEvent::VideoConfig(config) if self.transcode && self.renditions.is_none() => {
                config.avc()?;
                let source = VideoTrackInfo::from_config(config, "Video".to_string())?;
                self.renditions = RenditionFanout::run(&self.writer, &source);
                None
            }
            MediaEvent::Video(sample) => Some(sample.clone()),
            _ => None,
        };
        // The writer numbers a keyframe's group, which the renditions look up,
        // so it gets the sample before the transcoder.
        self.writer.push(event)?;
        if let (Some(renditions), Some(sample)) = (&self.renditions, rendition_sample) {
            renditions.push(sample);
        }
        Ok(())
    }
}
