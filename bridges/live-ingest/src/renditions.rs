use anyhow::Result;
use mediapack::{MediaEvent, VideoSample};
use tokio::{
    sync::mpsc,
    task::{self, JoinHandle},
};
use transcode::{Rendition, TranscodedEvent, Transcoder, ladder_for};

use crate::{
    moqt::{MoqtManager, VIDEO_TRACK_NAME, VideoTrackInfo},
    publisher::video_payload,
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
        let publisher = tokio::spawn(publish_renditions(transcoder, renditions, moqt, namespace));
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

async fn publish_renditions(
    mut transcoder: Transcoder,
    renditions: Vec<Rendition>,
    moqt: MoqtManager,
    namespace: Vec<String>,
) {
    let namespace_path = namespace.join("/");
    let mut codecs: Vec<Option<String>> = vec![None; renditions.len()];
    while let Some(output) = transcoder.next().await {
        let output = match output {
            Ok(output) => output,
            Err(err) => {
                tracing::error!(namespace = %namespace_path, ?err, "transcoder failed");
                break;
            }
        };
        let rendition = &renditions[output.rendition];
        let codec = &mut codecs[output.rendition];
        if let Err(err) = publish_output(&moqt, &namespace, rendition, codec, output).await {
            tracing::warn!(namespace = %namespace_path, rendition = %rendition.name, ?err, "failed to publish rendition");
        }
    }
}

async fn publish_output(
    moqt: &MoqtManager,
    namespace: &[String],
    rendition: &Rendition,
    codec: &mut Option<String>,
    output: TranscodedEvent,
) -> Result<()> {
    let track = format!("{VIDEO_TRACK_NAME}_{}", rendition.name);
    match output.event {
        MediaEvent::VideoConfig(config) => {
            let info = VideoTrackInfo::from_record(&config, format!("Video {}", rendition.name))?;
            *codec = Some(info.codec.clone());
            moqt.update_video_catalog(namespace, &track, info).await
        }
        MediaEvent::Video(sample) => {
            tracing::trace!(%track, pts = sample.pts.micros(), is_keyframe = sample.is_keyframe, "rendition sample received");
            let payload = video_payload(&sample, codec.as_deref());
            moqt.send_object(namespace, &track, sample.is_keyframe, payload)
                .await
        }
        _ => Ok(()),
    }
}
