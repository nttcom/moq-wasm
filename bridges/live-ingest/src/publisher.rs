use anyhow::Result;
use media_publisher::{MediaPublisher, MoqtManager, MoqtTarget, VideoTrackInfo};
use mediapack::MediaEvent;

use crate::renditions::RenditionFanout;

#[derive(Clone)]
pub struct IngestOptions {
    pub moqt: Option<MoqtTarget>,
    pub transcode: bool,
}

pub struct IngestPublisher {
    media: MediaPublisher,
    transcode: bool,
    renditions: Option<RenditionFanout>,
}

impl IngestPublisher {
    pub fn new(moqt: MoqtManager, namespace: Vec<String>, transcode: bool) -> Self {
        Self {
            media: MediaPublisher::run(moqt, namespace),
            transcode,
            renditions: None,
        }
    }

    pub fn push(&mut self, event: MediaEvent) -> Result<()> {
        match &event {
            MediaEvent::VideoConfig(config) if self.transcode && self.renditions.is_none() => {
                let source = VideoTrackInfo::from_record(config, "Video".to_string())?;
                self.renditions = RenditionFanout::run(&self.media, &source)?;
            }
            MediaEvent::Video(sample) => {
                if let Some(renditions) = &self.renditions {
                    renditions.push(sample);
                }
            }
            _ => {}
        }
        self.media.push(event)
    }
}
