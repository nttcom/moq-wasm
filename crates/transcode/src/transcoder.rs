use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, anyhow, ensure};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSinkCallbacks, AppSrc};
use mediapack::{MediaEvent, Timestamp, VideoSample, h264::ParameterSetTracker};
use tokio::sync::mpsc;

use crate::ladder::Rendition;

/// Renditions key only where the source does: the encoder's own keyframe
/// interval is pushed out of reach and a key unit is requested for the frame
/// that carries a source keyframe's presentation time, so every rendition group
/// starts on the same presentation time as the source group
/// (draft-ietf-moq-cmsf-01 §3.2). The request is made from a probe on the
/// encoder's sink pad as that frame arrives, and asks for the next frame rather
/// than for a running time: GstVideoEncoder 1.24 matches requested running
/// times against timestamps it has already shifted by the encoder's `min_pts`,
/// which x264enc sets to 1000 hours, so a request for the source time is never
/// honoured there.
const MAX_KEYFRAME_INTERVAL_FRAMES: u32 = i32::MAX as u32;
const INPUT_QUEUE_BYTES: u32 = 4 * 1024 * 1024;
/// A source keyframe whose frame never reaches an encoder (dropped in decode)
/// is forgotten once the encoder has moved this far past it.
const KEYFRAME_TIME_WINDOW_US: u64 = 10_000_000;

type KeyframeTimes = Arc<Mutex<BTreeSet<u64>>>;

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
    keyframe_times: Vec<KeyframeTimes>,
}

impl TranscodeInput {
    pub fn push(&self, sample: &VideoSample) -> Result<()> {
        if sample.is_keyframe {
            for keyframe_times in &self.keyframe_times {
                keyframe_times
                    .lock()
                    .expect("keyframe times lock")
                    .insert(sample.pts.micros());
            }
        }
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
        let mut keyframe_times = Vec::with_capacity(renditions.len());
        for index in 0..renditions.len() {
            let sink = pipeline
                .by_name(&format!("sink{index}"))
                .context("missing appsink")?
                .downcast::<AppSink>()
                .map_err(|_| anyhow!("sink{index} is not an appsink"))?;
            install_sink_callbacks(&sink, index, message_sender.clone());
            let encoder = pipeline
                .by_name(&format!("enc{index}"))
                .context("missing encoder")?;
            let times = KeyframeTimes::default();
            install_key_unit_probe(&encoder, times.clone())?;
            keyframe_times.push(times);
        }
        forward_bus_errors(&pipeline, message_sender)?;
        pipeline
            .set_state(gst::State::Playing)
            .context("start transcode pipeline")?;
        Ok(Self {
            pipeline,
            input: TranscodeInput {
                source,
                keyframe_times,
            },
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
             x264enc name=enc{index} tune=zerolatency speed-preset=veryfast bitrate={} key-int-max={MAX_KEYFRAME_INTERVAL_FRAMES} ! \
             video/x-h264,profile=baseline ! h264parse config-interval=-1 ! \
             video/x-h264,stream-format=byte-stream,alignment=au ! \
             appsink name=sink{index} sync=false ",
            rendition.width, rendition.height, rendition.bitrate_kbps
        ));
    }
    description
}

fn install_key_unit_probe(encoder: &gst::Element, keyframe_times: KeyframeTimes) -> Result<()> {
    let sink_pad = encoder
        .static_pad("sink")
        .context("encoder has no sink pad")?;
    let src_pad = encoder
        .static_pad("src")
        .context("encoder has no src pad")?;
    sink_pad
        .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            let Some(gst::PadProbeData::Buffer(buffer)) = &info.data else {
                return gst::PadProbeReturn::Ok;
            };
            let Some(pts) = buffer.pts() else {
                return gst::PadProbeReturn::Ok;
            };
            let pts = pts.useconds();
            let mut times = keyframe_times.lock().expect("keyframe times lock");
            *times = times.split_off(&pts.saturating_sub(KEYFRAME_TIME_WINDOW_US));
            if times.remove(&pts) {
                src_pad.send_event(force_key_unit_request());
            }
            gst::PadProbeReturn::Ok
        })
        .context("install key unit probe")?;
    Ok(())
}

fn force_key_unit_request() -> gst::Event {
    let request = gst::Structure::builder("GstForceKeyUnit")
        .field("running-time", gst::ClockTime::NONE)
        .field("all-headers", true)
        .field("count", 0u32)
        .build();
    gst::event::CustomUpstream::builder(request).build()
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
                let segment = sample
                    .segment()
                    .and_then(|segment| segment.downcast_ref::<gst::ClockTime>());
                let pts = running_timestamp(segment, buffer.pts());
                let dts = buffer
                    .dts()
                    .map_or(pts, |_| running_timestamp(segment, buffer.dts()));
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

/// Encoders shift PTS (and the segment with it) to keep DTS non-negative;
/// running time undoes that shift so outputs stay on the input timeline.
fn running_timestamp(
    segment: Option<&gst::FormattedSegment<gst::ClockTime>>,
    clock_time: Option<gst::ClockTime>,
) -> Timestamp {
    let running = clock_time.map(|time| {
        segment
            .and_then(|segment| segment.to_running_time(time))
            .unwrap_or(time)
    });
    Timestamp::from_micros(running.map_or(0, |time| time.useconds()))
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
        assert_eq!(video[0].pts, samples[0].pts);
        assert_eq!(video[0].dts, samples[0].pts);
        assert!(outputs.iter().all(|output| output.rendition == 0));
    }

    #[tokio::test]
    async fn keyframes_land_where_the_source_has_them() {
        // Arrange: the fixture played twice gives a source with keyframes past the first frame
        let samples = fixture_video_samples();
        let shift = samples
            .last()
            .unwrap()
            .pts
            .saturating_add(Timestamp::from_millis(40));
        let repeated: Vec<VideoSample> = samples
            .iter()
            .cloned()
            .chain(samples.iter().map(|sample| VideoSample {
                data: sample.data.clone(),
                is_keyframe: sample.is_keyframe,
                pts: sample.pts.saturating_add(shift),
                dts: sample.dts.saturating_add(shift),
            }))
            .collect();
        let source_keyframes: Vec<Timestamp> = repeated
            .iter()
            .filter(|sample| sample.is_keyframe)
            .map(|sample| sample.pts)
            .collect();
        assert!(source_keyframes.len() >= 2);
        let rendition = Rendition {
            name: "54p".into(),
            width: 96,
            height: 54,
            bitrate_kbps: 100,
        };
        let mut transcoder = Transcoder::new(&[rendition]).unwrap();

        // Act
        for sample in &repeated {
            transcoder.push(sample).unwrap();
        }
        transcoder.finish().unwrap();
        let outputs = tokio::time::timeout(Duration::from_secs(30), drain(&mut transcoder))
            .await
            .expect("transcoder finished");

        // Assert
        let output_keyframes: Vec<Timestamp> = outputs
            .iter()
            .filter_map(|output| match &output.event {
                MediaEvent::Video(sample) if sample.is_keyframe => Some(sample.pts),
                _ => None,
            })
            .collect();
        assert_eq!(output_keyframes, source_keyframes);
    }

    #[test]
    fn rejects_empty_ladder() {
        // Arrange / Act / Assert
        assert!(Transcoder::new(&[]).is_err());
    }
}
