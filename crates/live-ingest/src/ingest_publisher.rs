use anyhow::Result;
use broadcast::{Broadcast, BroadcastWriter, MoqtTarget, VideoTrackInfo};
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
            writer: BroadcastWriter::run(broadcast),
            transcode,
            renditions: None,
        }
    }

    pub fn push(&mut self, event: MediaEvent) -> Result<()> {
        match &event {
            MediaEvent::VideoConfig(config) if self.transcode && self.renditions.is_none() => {
                let source = VideoTrackInfo::from_record(config, "Video".to_string())?;
                self.renditions = RenditionFanout::run(&self.writer, &source)?;
            }
            MediaEvent::Video(sample) => {
                if let Some(renditions) = &self.renditions {
                    renditions.push(sample);
                }
            }
            _ => {}
        }
        self.writer.push(event)
    }
}
