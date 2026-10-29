use anyhow::Error;
use broadcast::{
    BroadcastWriter, RenditionConfig, RenditionTrack, VIDEO_TRACK_NAME, VideoTrackInfo,
};
use mediapack::VideoSample;
use tokio::{
    sync::mpsc,
    task::{self, JoinHandle},
};
use transcode::{Rendition, TranscodeInput, Transcoder, ladder_for};

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
        let tracks = renditions
            .iter()
            .map(|rendition| {
                RenditionTrack::new(
                    writer,
                    RenditionConfig {
                        track: format!("{VIDEO_TRACK_NAME}_{}", rendition.name),
                        label: format!("Video {}", rendition.name),
                        bitrate: Some(u64::from(rendition.bitrate_kbps) * 1000),
                        cmaf: true,
                    },
                )
            })
            .collect();
        let publisher = RenditionPublisher { tracks };
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
            publisher.run(transcoder, renditions, &namespace_path).await;
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
    tracks: Vec<RenditionTrack>,
}

impl RenditionPublisher {
    async fn run(
        mut self,
        mut transcoder: Transcoder,
        renditions: Vec<Rendition>,
        namespace_path: &str,
    ) {
        while let Some(output) = transcoder.next().await {
            let output = match output {
                Ok(output) => output,
                Err(err) => {
                    tracing::error!(namespace = %namespace_path, ?err, "transcoder failed");
                    break;
                }
            };
            let rendition = &renditions[output.rendition];
            let track = &mut self.tracks[output.rendition];
            if let Err(err) = track.push(output.event).await {
                tracing::warn!(namespace = %namespace_path, rendition = %rendition.name, ?err, "failed to publish rendition");
            }
        }
    }
}
