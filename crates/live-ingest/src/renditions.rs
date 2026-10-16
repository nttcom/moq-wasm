use anyhow::{Context, Error, Result};
use broadcast::{
    Broadcast, BroadcastWriter, GroupAlignment, GroupBoundary, OutgoingObject, SharedTiming,
    VIDEO_TRACK_NAME, VideoTrackInfo, cmaf_track_name,
};
use mediapack::{MediaEvent, VideoSample, loc::to_extension_headers, mp4::Fmp4TrackMuxer};
use tokio::{
    sync::mpsc,
    task::{self, JoinHandle},
};
use transcode::{Rendition, TranscodeInput, TranscodedEvent, Transcoder, ladder_for};

const SAMPLE_QUEUE_CAPACITY: usize = 64;

pub struct RenditionFanout {
    sample_sender: mpsc::Sender<VideoSample>,
    _task: JoinHandle<()>,
}

impl RenditionFanout {
    pub fn run(writer: &BroadcastWriter, source: &VideoTrackInfo) -> Option<Self> {
        let broadcast = writer.broadcast().clone();
        let namespace_path = broadcast.namespace().join("/");
        let renditions = ladder_for(source.width, source.height);
        if renditions.is_empty() {
            tracing::info!(namespace = %namespace_path, width = source.width, height = source.height, "source too small for lower renditions");
            return None;
        }
        let (sample_sender, sample_receiver) = mpsc::channel::<VideoSample>(SAMPLE_QUEUE_CAPACITY);
        let publisher = RenditionPublisher {
            broadcast,
            timing: writer.shared_timing(),
            cmaf: (0..renditions.len()).map(|_| None).collect(),
        };
        let task = tokio::spawn(async move {
            // Building the pipeline loads the GStreamer plugins. It runs off the
            // input's task because a stall there delays the first sample's push,
            // which takes the capture origin late and stamps the backlog read
            // after it into the future.
            let built = task::spawn_blocking(move || {
                Transcoder::new(&renditions).map(|transcoder| (transcoder, renditions))
            })
            .await;
            let (transcoder, renditions) = match built.map_err(Error::from).and_then(|built| built)
            {
                Ok(built) => built,
                Err(err) => {
                    tracing::error!(namespace = %namespace_path, ?err, "failed to start the transcoder");
                    return;
                }
            };
            tracing::info!(
                namespace = %namespace_path,
                renditions = ?renditions.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
                "transcoding renditions started"
            );
            let feeder = feed(transcoder.input(), sample_receiver);
            publisher.run(transcoder, renditions).await;
            let _ = feeder.await;
        });
        Some(Self {
            sample_sender,
            _task: task,
        })
    }

    pub fn push(&self, sample: &VideoSample) {
        if self.sample_sender.try_send(sample.clone()).is_err() {
            tracing::warn!("transcoder input queue full, dropping video sample");
        }
    }
}

fn feed(input: TranscodeInput, mut sample_receiver: mpsc::Receiver<VideoSample>) -> JoinHandle<()> {
    task::spawn_blocking(move || {
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
    })
}

struct RenditionPublisher {
    broadcast: Broadcast,
    timing: SharedTiming,
    cmaf: Vec<Option<Fmp4TrackMuxer>>,
}

impl RenditionPublisher {
    async fn run(mut self, mut transcoder: Transcoder, renditions: Vec<Rendition>) {
        let namespace_path = self.broadcast.namespace().join("/");
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
                self.cmaf[index] = Some(Fmp4TrackMuxer::video(config.clone()));
                let mut info =
                    VideoTrackInfo::from_record(&config, format!("Video {}", rendition.name))?;
                info.bitrate = Some(u64::from(rendition.bitrate_kbps) * 1000);
                self.broadcast.update_video_catalog(&track, info).await
            }
            MediaEvent::Video(sample) => {
                tracing::trace!(%track, pts = sample.pts.micros(), is_keyframe = sample.is_keyframe, "rendition sample received");
                if sample.is_keyframe
                    && self.timing.alignment.aligned(sample.pts.micros()).is_none()
                {
                    tracing::warn!(
                        %track,
                        pts_us = sample.pts.micros(),
                        nearest_source_keyframe_us = ?self.timing.alignment.nearest_keyframe_us(sample.pts.micros()),
                        "rendition keyframe has no aligned source keyframe; its frames are dropped until one aligns"
                    );
                }
                let fragment = match &mut self.cmaf[index] {
                    Some(muxer) => muxer.push(&MediaEvent::Video(sample.clone()))?,
                    None => None,
                };
                let group = aligned_boundary(
                    &self.timing.alignment,
                    sample.is_keyframe,
                    sample.pts.micros(),
                );
                let muxer =
                    self.timing.loc.get().context(
                        "rendition sample before the source seeded the LOC capture origin",
                    )?;
                if let Some(object) = muxer.push(&MediaEvent::Video(sample)) {
                    self.broadcast
                        .send_object(
                            &track,
                            OutgoingObject {
                                group,
                                extension_headers: to_extension_headers(&object.extensions),
                                payload: object.payload,
                            },
                        )
                        .await?;
                }
                let Some(fragment) = fragment else {
                    return Ok(());
                };
                let group = aligned_boundary(
                    &self.timing.alignment,
                    fragment.is_keyframe,
                    fragment.presentation_time.micros(),
                );
                self.broadcast
                    .send_object(
                        &cmaf_track_name(&track),
                        OutgoingObject::plain(group, fragment.data),
                    )
                    .await?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

fn aligned_boundary(
    alignment: &GroupAlignment,
    is_keyframe: bool,
    presentation_us: u64,
) -> GroupBoundary {
    match (is_keyframe, alignment.aligned(presentation_us)) {
        (true, Some(group_id)) => GroupBoundary::At(group_id),
        _ => GroupBoundary::Within,
    }
}
