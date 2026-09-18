use anyhow::{Context, Result};
use media_publisher::{
    GroupAlignment, GroupBoundary, MediaPublisher, MoqtManager, OutgoingObject, SharedTiming,
    VIDEO_TRACK_NAME, VideoTrackInfo, cmaf_track_name,
};
use mediapack::{MediaEvent, VideoSample, loc::to_extension_headers, mp4::Fmp4TrackMuxer};
use tokio::{
    sync::mpsc,
    task::{self, JoinHandle},
};
use transcode::{Rendition, TranscodedEvent, Transcoder, ladder_for};

const SAMPLE_QUEUE_CAPACITY: usize = 64;

pub struct RenditionFanout {
    sample_sender: mpsc::Sender<VideoSample>,
    _feeder: JoinHandle<()>,
    _publisher: JoinHandle<()>,
}

impl RenditionFanout {
    pub fn run(media: &MediaPublisher, source: &VideoTrackInfo) -> Result<Option<Self>> {
        let namespace = media.namespace().to_vec();
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
            moqt: media.moqt().clone(),
            namespace,
            timing: media.shared_timing(),
            cmaf: (0..renditions.len()).map(|_| None).collect(),
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
    timing: SharedTiming,
    cmaf: Vec<Option<Fmp4TrackMuxer>>,
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
                self.cmaf[index] = Some(Fmp4TrackMuxer::video(config.clone()));
                let info =
                    VideoTrackInfo::from_record(&config, format!("Video {}", rendition.name))?;
                self.moqt
                    .update_video_catalog(&self.namespace, &track, info)
                    .await
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
                    self.moqt
                        .send_object(
                            &self.namespace,
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
                self.moqt
                    .send_object(
                        &self.namespace,
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
