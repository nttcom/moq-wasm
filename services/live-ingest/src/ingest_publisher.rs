use anyhow::Result;
use broadcast::{Broadcast, BroadcastWriter, MoqtTarget, SourceTracks, VideoTrackInfo};
use mediapack::MediaEvent;

use crate::{renditions::RenditionFanout, stats_panel::ConnectionRegistry};

#[derive(Clone)]
pub struct IngestOptions {
    pub moqt: Option<MoqtTarget>,
    pub transcode: bool,
    pub stats: Option<ConnectionRegistry>,
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
