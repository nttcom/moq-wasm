use anyhow::{bail, Context, Result};
use bytes::Bytes;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};
use mediapack::aac::AudioSpecificConfig;
use mediapack::h264::ParameterSetTracker;
use mediapack::{AudioConfig, AudioSample, MediaEvent, Timestamp};
use tokio::sync::mpsc::Sender as TokioSender;

const SINK_NAME: &str = "sink";
// h264parse config-interval=-1 puts SPS/PPS in front of every IDR, also when the
// camera announces them only in the SDP, so each group starts decodable.
const H264_BRANCH: &str = "rtph264depay ! h264parse config-interval=-1 \
    ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=sink sync=false";
const PCMA_BRANCH: &str = "rtppcmadepay ! appsink name=sink sync=false";
const AAC_BRANCH: &str =
    "rtpmp4gdepay ! aacparse ! audio/mpeg,stream-format=raw ! appsink name=sink sync=false";
const OPUS_BRANCH: &str = "rtpopusdepay ! appsink name=sink sync=false";
const UNSUPPORTED_BRANCH: &str = "fakesink sync=false";
const OPUS_SAMPLE_RATE: u32 = 48_000;

/// Blocks the calling thread playing the RTSP stream at `url` and sending its
/// media as `MediaEvent`s; returns an error once the stream ends or fails.
pub fn run(url: &str, event_sender: TokioSender<MediaEvent>) -> Result<()> {
    gst::init().context("initialize GStreamer")?;
    let pipeline = gst::Pipeline::new();
    let source = gst::ElementFactory::make("rtspsrc")
        .property("location", url)
        .property("latency", 0u32)
        .build()
        .context("create rtspsrc (GStreamer good plugins)")?;
    source.set_property_from_str("protocols", "tcp");
    pipeline.add(&source)?;
    let pipeline_weak = pipeline.downgrade();
    source.connect_pad_added(move |_, pad| {
        let Some(pipeline) = pipeline_weak.upgrade() else {
            return;
        };
        if let Err(err) = attach_branch(&pipeline, pad, &event_sender) {
            log::warn!("RTSP stream not attached: {err:#}");
        }
    });
    pipeline
        .set_state(gst::State::Playing)
        .context("start RTSP pipeline")?;
    let result = wait_for_end(&pipeline.bus().context("RTSP pipeline has no bus")?);
    let _ = pipeline.set_state(gst::State::Null);
    result
}

fn attach_branch(
    pipeline: &gst::Pipeline,
    pad: &gst::Pad,
    event_sender: &TokioSender<MediaEvent>,
) -> Result<()> {
    let caps = pad.current_caps().context("RTSP pad has no caps")?;
    let encoding = caps
        .structure(0)
        .and_then(|structure| structure.get::<&str>("encoding-name").ok())
        .unwrap_or_default()
        .to_owned();
    let description = branch_description(&encoding);
    let callbacks = match description {
        H264_BRANCH => {
            let mut tracker = ParameterSetTracker::new();
            Some(appsink_callbacks(
                event_sender.clone(),
                move |_, data, timing| video_events(&mut tracker, data, timing),
            ))
        }
        UNSUPPORTED_BRANCH => {
            log::warn!("RTSP stream with encoding {encoding:?} is not bridged");
            None
        }
        _ => {
            let mut config = None;
            Some(appsink_callbacks(
                event_sender.clone(),
                move |sample, data, timing| audio_events(&mut config, sample, data, timing),
            ))
        }
    };
    let branch = gst::parse::bin_from_description(description, true)
        .with_context(|| format!("build RTSP branch for {encoding}"))?;
    if let Some(callbacks) = callbacks {
        branch
            .by_name(SINK_NAME)
            .and_then(|sink| sink.downcast::<AppSink>().ok())
            .context("RTSP branch has no appsink")?
            .set_callbacks(callbacks);
    }
    pipeline.add(&branch)?;
    branch.sync_state_with_parent()?;
    pad.link(
        &branch
            .static_pad("sink")
            .context("RTSP branch has no sink pad")?,
    )?;
    log::info!("RTSP stream bridged: encoding={encoding}");
    Ok(())
}

fn branch_description(encoding: &str) -> &'static str {
    match encoding {
        "H264" => H264_BRANCH,
        "PCMA" => PCMA_BRANCH,
        "MPEG4-GENERIC" => AAC_BRANCH,
        "OPUS" => OPUS_BRANCH,
        _ => UNSUPPORTED_BRANCH,
    }
}

fn appsink_callbacks(
    event_sender: TokioSender<MediaEvent>,
    mut to_events: impl FnMut(&gst::Sample, &[u8], MediaTiming) -> Result<Vec<MediaEvent>>
        + Send
        + 'static,
) -> AppSinkCallbacks {
    AppSinkCallbacks::builder()
        .new_sample(move |sink| {
            let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
            let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
            let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
            match to_events(&sample, map.as_slice(), MediaTiming::of(buffer)) {
                Ok(events) => {
                    for event in events {
                        event_sender
                            .blocking_send(event)
                            .map_err(|_| gst::FlowError::Eos)?;
                    }
                }
                Err(err) => log::warn!("RTSP sample dropped: {err:#}"),
            }
            Ok(gst::FlowSuccess::Ok)
        })
        .build()
}

fn video_events(
    tracker: &mut ParameterSetTracker,
    access_unit: &[u8],
    timing: MediaTiming,
) -> Result<Vec<MediaEvent>> {
    Ok(tracker
        .track(access_unit)?
        .map(|unit| unit.into_events(timing.pts, timing.dts))
        .unwrap_or_default())
}

fn audio_events(
    current: &mut Option<AudioConfig>,
    sample: &gst::Sample,
    data: &[u8],
    timing: MediaTiming,
) -> Result<Vec<MediaEvent>> {
    let config = audio_config(sample.caps().context("audio sample without caps")?)?;
    let mut events = Vec::with_capacity(2);
    if current.as_ref() != Some(&config) {
        *current = Some(config.clone());
        events.push(MediaEvent::AudioConfig(config));
    }
    events.push(MediaEvent::Audio(AudioSample {
        data: Bytes::copy_from_slice(data),
        pts: timing.pts,
    }));
    Ok(events)
}

fn audio_config(caps: &gst::CapsRef) -> Result<AudioConfig> {
    let structure = caps.structure(0).context("audio caps are empty")?;
    let rate = structure
        .get::<i32>("rate")
        .ok()
        .and_then(|rate| u32::try_from(rate).ok());
    let channels = structure
        .get::<i32>("channels")
        .ok()
        .and_then(|channels| u8::try_from(channels).ok())
        .unwrap_or(1);
    match structure.name().as_str() {
        "audio/x-alaw" => Ok(AudioConfig::Pcma {
            sample_rate: rate.context("A-law caps without a rate")?,
            channels,
        }),
        "audio/x-opus" => Ok(AudioConfig::Opus {
            sample_rate: rate.unwrap_or(OPUS_SAMPLE_RATE),
            channels,
        }),
        "audio/mpeg" => {
            let codec_data = structure
                .get::<gst::Buffer>("codec_data")
                .context("AAC caps without codec_data")?;
            let codec_data = codec_data.map_readable()?;
            Ok(AudioConfig::Aac(AudioSpecificConfig::parse(
                codec_data.as_slice(),
            )?))
        }
        other => bail!("unsupported audio caps {other}"),
    }
}

fn wait_for_end(bus: &gst::Bus) -> Result<()> {
    for message in bus.iter_timed(gst::ClockTime::NONE) {
        match message.view() {
            gst::MessageView::Eos(_) => bail!("RTSP stream ended"),
            gst::MessageView::Error(error) => bail!(
                "RTSP pipeline error from {}: {} ({})",
                error
                    .src()
                    .map(|src| src.path_string().to_string())
                    .unwrap_or_default(),
                error.error(),
                error.debug().unwrap_or_default()
            ),
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct MediaTiming {
    pts: Timestamp,
    dts: Timestamp,
}

impl MediaTiming {
    fn of(buffer: &gst::BufferRef) -> Self {
        let pts = Timestamp::from_micros(buffer.pts().map_or(0, |pts| pts.useconds()));
        let dts = buffer
            .dts()
            .map_or(pts, |dts| Timestamp::from_micros(dts.useconds()));
        Self { pts, dts }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: [u8; 24] = [
        0x67, 0x42, 0xd0, 0x0b, 0xda, 0x0a, 0x37, 0xe4, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00, 0x04,
        0x00, 0x00, 0x03, 0x00, 0x78, 0x3c, 0x48, 0x9a, 0x80,
    ];
    const PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];
    const IDR_SLICE: [u8; 3] = [0x65, 0x88, 0x84];
    const START_CODE: [u8; 4] = [0, 0, 0, 1];
    const TIMING: MediaTiming = MediaTiming {
        pts: Timestamp::from_micros(1_000),
        dts: Timestamp::from_micros(1_000),
    };

    fn keyframe() -> Vec<u8> {
        [&SPS[..], &PPS[..], &IDR_SLICE[..]]
            .iter()
            .flat_map(|nal| START_CODE.iter().chain(nal.iter()).copied())
            .collect()
    }

    fn audio_events_with(caps: gst::Caps) -> Vec<MediaEvent> {
        let sample = gst::Sample::builder().caps(&caps).build();
        audio_events(&mut None, &sample, &[1, 2], TIMING).unwrap()
    }

    #[test]
    fn a_keyframe_brings_the_video_config_before_the_sample() {
        // Arrange
        let mut tracker = ParameterSetTracker::new();

        // Act
        let events = video_events(&mut tracker, &keyframe(), TIMING).unwrap();

        // Assert
        assert!(matches!(&events[0], MediaEvent::VideoConfig(_)));
        assert!(matches!(
            &events[1],
            MediaEvent::Video(sample) if sample.is_keyframe && sample.pts == TIMING.pts
        ));
    }

    #[test]
    fn alaw_caps_become_pcma() {
        // Arrange
        gst::init().unwrap();
        let caps = gst::Caps::builder("audio/x-alaw")
            .field("rate", 8_000i32)
            .field("channels", 1i32)
            .build();

        // Act
        let events = audio_events_with(caps);

        // Assert
        assert_eq!(
            events[0],
            MediaEvent::AudioConfig(AudioConfig::Pcma {
                sample_rate: 8_000,
                channels: 1
            })
        );
        assert_eq!(
            events[1],
            MediaEvent::Audio(AudioSample {
                data: Bytes::from_static(&[1, 2]),
                pts: TIMING.pts
            })
        );
    }

    #[test]
    fn aac_caps_take_the_config_from_codec_data() {
        // Arrange
        gst::init().unwrap();
        let config = AudioSpecificConfig::new(2, 48_000, 2);
        let caps = gst::Caps::builder("audio/mpeg")
            .field("mpegversion", 4i32)
            .field("codec_data", gst::Buffer::from_slice(config.to_bytes()))
            .build();

        // Act
        let events = audio_events_with(caps);

        // Assert
        assert_eq!(events[0], MediaEvent::AudioConfig(AudioConfig::Aac(config)));
    }

    #[test]
    fn an_unchanged_audio_config_is_not_repeated() {
        // Arrange
        gst::init().unwrap();
        let caps = gst::Caps::builder("audio/x-opus")
            .field("channels", 2i32)
            .build();
        let sample = gst::Sample::builder().caps(&caps).build();
        let mut current = None;
        audio_events(&mut current, &sample, &[1], TIMING).unwrap();

        // Act
        let events = audio_events(&mut current, &sample, &[2], TIMING).unwrap();

        // Assert
        assert_eq!(
            current,
            Some(AudioConfig::Opus {
                sample_rate: 48_000,
                channels: 2
            })
        );
        assert!(matches!(events.as_slice(), [MediaEvent::Audio(_)]));
    }

    #[test]
    fn every_bridged_encoding_builds_a_branch_with_an_appsink() {
        // Arrange
        gst::init().unwrap();

        for encoding in ["H264", "PCMA", "MPEG4-GENERIC", "OPUS"] {
            // Act
            let branch =
                gst::parse::bin_from_description(branch_description(encoding), true).unwrap();

            // Assert
            assert!(
                branch
                    .by_name(SINK_NAME)
                    .is_some_and(|sink| sink.is::<AppSink>()),
                "{encoding}"
            );
        }
    }
}
