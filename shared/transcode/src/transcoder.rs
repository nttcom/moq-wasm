use anyhow::{Context, Result, anyhow, ensure};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSinkCallbacks, AppSrc};
use mediapack::{MediaEvent, Timestamp, VideoSample, h264::ParameterSetTracker};
use tokio::sync::mpsc;

use crate::ladder::Rendition;

const KEYFRAME_INTERVAL_FRAMES: u32 = 60;
const INPUT_QUEUE_BYTES: u32 = 4 * 1024 * 1024;

pub struct TranscodedEvent {
    pub rendition: usize,
    pub event: MediaEvent,
}

enum Message {
    Event(TranscodedEvent),
    Error(anyhow::Error),
    EndOfStream,
}

pub struct Transcoder {
    pipeline: gst::Pipeline,
    input: TranscodeInput,
    message_receiver: mpsc::UnboundedReceiver<Message>,
    open_renditions: usize,
}

#[derive(Clone)]
pub struct TranscodeInput {
    source: AppSrc,
}

impl TranscodeInput {
    pub fn push(&self, sample: &VideoSample) -> Result<()> {
        let mut buffer = gst::Buffer::from_slice(sample.data.clone());
        {
            let buffer = buffer
                .get_mut()
                .ok_or_else(|| anyhow!("fresh buffer is shared"))?;
            buffer.set_pts(gst::ClockTime::from_useconds(sample.pts.micros()));
            buffer.set_dts(gst::ClockTime::from_useconds(sample.dts.micros()));
            if !sample.is_keyframe {
                buffer.set_flags(gst::BufferFlags::DELTA_UNIT);
            }
        }
        self.source
            .push_buffer(buffer)
            .map(|_| ())
            .map_err(|error| anyhow!("push sample into transcoder: {error:?}"))
    }

    pub fn finish(&self) -> Result<()> {
        self.source
            .end_of_stream()
            .map(|_| ())
            .map_err(|error| anyhow!("signal end of stream: {error:?}"))
    }
}

impl Transcoder {
    pub fn new(renditions: &[Rendition]) -> Result<Self> {
        ensure!(!renditions.is_empty(), "at least one rendition is required");
        gst::init()?;
        let pipeline = gst::parse::launch(&pipeline_description(renditions))
            .context("build transcode pipeline")?
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow!("transcode description did not produce a pipeline"))?;
        let source = pipeline
            .by_name("source")
            .context("missing appsrc")?
            .downcast::<AppSrc>()
            .map_err(|_| anyhow!("source is not an appsrc"))?;
        let (message_sender, message_receiver) = mpsc::unbounded_channel();
        for index in 0..renditions.len() {
            let sink = pipeline
                .by_name(&format!("sink{index}"))
                .context("missing appsink")?
                .downcast::<AppSink>()
                .map_err(|_| anyhow!("sink{index} is not an appsink"))?;
            install_sink_callbacks(&sink, index, message_sender.clone());
        }
        forward_bus_errors(&pipeline, message_sender)?;
        pipeline
            .set_state(gst::State::Playing)
            .context("start transcode pipeline")?;
        Ok(Self {
            pipeline,
            input: TranscodeInput { source },
            message_receiver,
            open_renditions: renditions.len(),
        })
    }

    pub fn input(&self) -> TranscodeInput {
        self.input.clone()
    }

    pub fn push(&self, sample: &VideoSample) -> Result<()> {
        self.input.push(sample)
    }

    pub fn finish(&self) -> Result<()> {
        self.input.finish()
    }

    pub async fn next(&mut self) -> Option<Result<TranscodedEvent>> {
        while self.open_renditions > 0 {
            match self.message_receiver.recv().await? {
                Message::Event(event) => return Some(Ok(event)),
                Message::Error(error) => return Some(Err(error)),
                Message::EndOfStream => self.open_renditions -= 1,
            }
        }
        None
    }
}

impl Drop for Transcoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn pipeline_description(renditions: &[Rendition]) -> String {
    let mut description = format!(
        "appsrc name=source is-live=true format=time block=true max-bytes={INPUT_QUEUE_BYTES} \
         caps=\"video/x-h264,stream-format=(string)byte-stream,alignment=(string)au\" ! \
         h264parse ! decodebin ! videoconvert ! tee name=split "
    );
    for (index, rendition) in renditions.iter().enumerate() {
        description.push_str(&format!(
            "split. ! queue ! videoscale ! video/x-raw,width={},height={} ! \
             x264enc tune=zerolatency speed-preset=veryfast bitrate={} key-int-max={KEYFRAME_INTERVAL_FRAMES} ! \
             video/x-h264,profile=baseline ! h264parse config-interval=-1 ! \
             video/x-h264,stream-format=byte-stream,alignment=au ! \
             appsink name=sink{index} sync=false ",
            rendition.width, rendition.height, rendition.bitrate_kbps
        ));
    }
    description
}

fn install_sink_callbacks(
    sink: &AppSink,
    rendition: usize,
    message_sender: mpsc::UnboundedSender<Message>,
) {
    let eos_sender = message_sender.clone();
    let mut parameter_sets = ParameterSetTracker::new();
    sink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let pts = timestamp(buffer.pts());
                let dts = buffer.dts().map_or(pts, |_| timestamp(buffer.dts()));
                match parameter_sets.track(map.as_slice()) {
                    Ok(Some(unit)) => {
                        if let Some(config) = unit.config_changed {
                            let _ = message_sender.send(Message::Event(TranscodedEvent {
                                rendition,
                                event: MediaEvent::VideoConfig(config),
                            }));
                        }
                        let _ = message_sender.send(Message::Event(TranscodedEvent {
                            rendition,
                            event: MediaEvent::Video(VideoSample {
                                data: unit.data,
                                is_keyframe: unit.is_keyframe,
                                pts,
                                dts,
                            }),
                        }));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = message_sender.send(Message::Error(error));
                    }
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .eos(move |_| {
                let _ = eos_sender.send(Message::EndOfStream);
            })
            .build(),
    );
}

fn forward_bus_errors(
    pipeline: &gst::Pipeline,
    message_sender: mpsc::UnboundedSender<Message>,
) -> Result<()> {
    let bus = pipeline.bus().context("transcode pipeline has no bus")?;
    bus.set_sync_handler(move |_, message| {
        if let gst::MessageView::Error(error) = message.view() {
            let _ = message_sender.send(Message::Error(anyhow!(
                "transcode pipeline error: {} ({:?})",
                error.error(),
                error.debug()
            )));
        }
        gst::BusSyncReply::Drop
    });
    Ok(())
}

fn timestamp(clock_time: Option<gst::ClockTime>) -> Timestamp {
    Timestamp::from_micros(clock_time.map_or(0, |time| time.useconds()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mediapack::mpegts;

    use super::*;

    const FIXTURE_TS: &[u8] = include_bytes!("../../mediapack/fixtures/testsrc.ts");

    fn fixture_video_samples() -> Vec<VideoSample> {
        let mut demuxer = mpegts::Demuxer::new();
        let mut events = demuxer.push(FIXTURE_TS).unwrap();
        events.extend(demuxer.finish().unwrap());
        events
            .into_iter()
            .filter_map(|event| match event {
                MediaEvent::Video(sample) => Some(sample),
                _ => None,
            })
            .collect()
    }

    async fn drain(transcoder: &mut Transcoder) -> Vec<TranscodedEvent> {
        let mut outputs = Vec::new();
        while let Some(event) = transcoder.next().await {
            outputs.push(event.unwrap());
        }
        outputs
    }

    #[tokio::test]
    async fn transcodes_fixture_into_a_smaller_rendition() {
        // Arrange
        let samples = fixture_video_samples();
        let rendition = Rendition {
            name: "54p".into(),
            width: 96,
            height: 54,
            bitrate_kbps: 100,
        };
        let mut transcoder = Transcoder::new(&[rendition]).unwrap();

        // Act
        for sample in &samples {
            transcoder.push(sample).unwrap();
        }
        transcoder.finish().unwrap();
        let outputs = tokio::time::timeout(Duration::from_secs(30), drain(&mut transcoder))
            .await
            .expect("transcoder finished");

        // Assert
        let MediaEvent::VideoConfig(config) = &outputs[0].event else {
            panic!("first event must be the rendition's VideoConfig");
        };
        let sps = config.sequence_parameter_set().unwrap();
        assert_eq!((sps.width, sps.height), (96, 54));
        let video: Vec<&VideoSample> = outputs
            .iter()
            .filter_map(|output| match &output.event {
                MediaEvent::Video(sample) => Some(sample),
                _ => None,
            })
            .collect();
        assert_eq!(video.len(), samples.len());
        assert!(video[0].is_keyframe);
        assert!(video.windows(2).all(|pair| pair[0].pts < pair[1].pts));
        assert!(outputs.iter().all(|output| output.rendition == 0));
    }

    #[test]
    fn rejects_empty_ladder() {
        // Arrange / Act / Assert
        assert!(Transcoder::new(&[]).is_err());
    }
}
