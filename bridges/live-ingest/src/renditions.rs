use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use mediapack::{MediaEvent, VideoSample, loc::Muxer as LocMuxer};
use tokio::{
    sync::mpsc,
    task::{self, JoinHandle},
};
use transcode::{Rendition, TranscodedEvent, Transcoder, ladder_for};

use crate::{
    group_alignment::GroupAlignment,
    loc_object::extension_headers,
    moqt::{GroupBoundary, MoqtManager, OutgoingObject, VIDEO_TRACK_NAME, VideoTrackInfo},
};

const SAMPLE_QUEUE_CAPACITY: usize = 64;

pub struct RenditionFanout {
    sample_sender: mpsc::Sender<VideoSample>,
    _feeder: JoinHandle<()>,
    _publisher: JoinHandle<()>,
}

impl RenditionFanout {
    pub fn run(
        moqt: MoqtManager,
        namespace: Vec<String>,
        source: &VideoTrackInfo,
        loc: Arc<OnceLock<LocMuxer>>,
        alignment: Arc<GroupAlignment>,
    ) -> Result<Option<Self>> {
        let namespace_path = namespace.join("/");
        let renditions = ladder_for(source.width, source.height);
        if renditions.is_empty() {
            tracing::info!(namespace = %namespace_path, width = source.width, height = source.height, "source too small for lower renditions");
            return Ok(None);
        }
        let transcoder = Transcoder::new(&renditions)?;
        tracing::info!(
            namespace = %namespace_path,
            renditions = ?renditions.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            "transcoding renditions started"
        );
        let input = transcoder.input();
        let (sample_sender, mut sample_receiver) =
            mpsc::channel::<VideoSample>(SAMPLE_QUEUE_CAPACITY);
        let feeder = task::spawn_blocking(move || {
            while let Some(sample) = sample_receiver.blocking_recv() {
                tracing::trace!(pts = sample.pts.micros(), "feeding transcoder");
                if let Err(err) = input.push(&sample) {
                    tracing::warn!(?err, "failed to feed the transcoder");
                    break;
                }
            }
            if let Err(err) = input.finish() {
                tracing::warn!(?err, "failed to finish the transcoder");
            }
        });
        let publisher = RenditionPublisher {
            moqt,
            namespace,
            loc,
            alignment,
            started: vec![false; renditions.len()],
        };
        let publisher = tokio::spawn(publisher.run(transcoder, renditions));
        Ok(Some(Self {
            sample_sender,
            _feeder: feeder,
            _publisher: publisher,
        }))
    }

    pub fn push(&self, sample: &VideoSample) {
        if self.sample_sender.try_send(sample.clone()).is_err() {
            tracing::warn!("transcoder input queue full, dropping video sample");
        }
    }
}

struct RenditionPublisher {
    moqt: MoqtManager,
    namespace: Vec<String>,
    loc: Arc<OnceLock<LocMuxer>>,
    alignment: Arc<GroupAlignment>,
    /// A rendition publishes nothing until a keyframe the source has an id for,
    /// so its first group starts aligned rather than on a lone delta frame.
    started: Vec<bool>,
}

impl RenditionPublisher {
    async fn run(mut self, mut transcoder: Transcoder, renditions: Vec<Rendition>) {
        let namespace_path = self.namespace.join("/");
        while let Some(output) = transcoder.next().await {
            let output = match output {
                Ok(output) => output,
                Err(err) => {
                    tracing::error!(namespace = %namespace_path, ?err, "transcoder failed");
                    break;
                }
            };
            let rendition = &renditions[output.rendition];
            if let Err(err) = self.publish(rendition, output).await {
                tracing::warn!(namespace = %namespace_path, rendition = %rendition.name, ?err, "failed to publish rendition");
            }
        }
    }

    async fn publish(&mut self, rendition: &Rendition, output: TranscodedEvent) -> Result<()> {
        let track = format!("{VIDEO_TRACK_NAME}_{}", rendition.name);
        let index = output.rendition;
        match output.event {
            MediaEvent::VideoConfig(config) => {
                let info =
                    VideoTrackInfo::from_record(&config, format!("Video {}", rendition.name))?;
                self.moqt
                    .update_video_catalog(&self.namespace, &track, info)
                    .await
            }
            MediaEvent::Video(sample) => {
                tracing::trace!(%track, pts = sample.pts.micros(), is_keyframe = sample.is_keyframe, "rendition sample received");
                let group = match (
                    sample.is_keyframe,
                    self.alignment.aligned(sample.pts.micros()),
                ) {
                    (true, Some(group_id)) => {
                        self.started[index] = true;
                        GroupBoundary::At(group_id)
                    }
                    _ if self.started[index] => GroupBoundary::Within,
                    _ => return Ok(()),
                };
                let muxer = self
                    .loc
                    .get()
                    .context("rendition sample before the source seeded the LOC capture origin")?;
                let Some(object) = muxer.push(&MediaEvent::Video(sample)) else {
                    return Ok(());
                };
                self.moqt
                    .send_object(
                        &self.namespace,
                        &track,
                        OutgoingObject {
                            group,
                            extension_headers: extension_headers(&object),
                            payload: object.payload,
                        },
                    )
                    .await
            }
            _ => Ok(()),
        }
    }
}
