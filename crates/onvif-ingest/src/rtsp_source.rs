use crate::rtsp_frame::{EncodedAudioPacket, EncodedPacket, RtspPacket};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};
use mediapack::aac::AudioSpecificConfig;
use mediapack::h264::{annexb_to_avcc, AvcDecoderConfigurationRecord, ParameterSetTracker};
use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::Sender as TokioSender;

const SINK_NAME: &str = "sink";
const NAL_LENGTH_SIZE: usize = 4;
// h264parse config-interval=-1 puts SPS/PPS in front of every IDR, also when the
// camera announces them only in the SDP, so each group starts decodable.
const H264_BRANCH: &str = "rtph264depay ! h264parse config-interval=-1 \
    ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=sink sync=false";
const PCMA_BRANCH: &str = "rtppcmadepay ! appsink name=sink sync=false";
const AAC_BRANCH: &str =
    "rtpmp4gdepay ! aacparse ! audio/mpeg,stream-format=raw ! appsink name=sink sync=false";
const OPUS_BRANCH: &str = "rtpopusdepay ! appsink name=sink sync=false";
const UNSUPPORTED_BRANCH: &str = "fakesink sync=false";

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum PayloadFormat {
    #[value(name = "annexb")]
    AnnexB,
    Avcc,
}

pub fn run(
    url: String,
    fallback_video_codec: String,
    payload_format: PayloadFormat,
    packet_sender: TokioSender<RtspPacket>,
    error_sender: Sender<String>,
) {
    if let Err(err) = stream(&url, payload_format, fallback_video_codec, packet_sender) {
        let _ = error_sender.send(format!("{err:#}"));
    }
}

fn stream(
    url: &str,
    payload_format: PayloadFormat,
    fallback_video_codec: String,
    packet_sender: TokioSender<RtspPacket>,
) -> Result<()> {
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
        let video = VideoPacketizer::new(payload_format, fallback_video_codec.clone());
        if let Err(err) = attach_branch(&pipeline, pad, video, &packet_sender) {
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
    mut video: VideoPacketizer,
    packet_sender: &TokioSender<RtspPacket>,
) -> Result<()> {
    let caps = pad.current_caps().context("RTSP pad has no caps")?;
    let encoding = caps
        .structure(0)
        .and_then(|structure| structure.get::<&str>("encoding-name").ok())
        .unwrap_or_default()
        .to_owned();
    let (description, callbacks) = match branch_description(&encoding) {
        H264_BRANCH => (
            H264_BRANCH,
            Some(appsink_callbacks(
                packet_sender.clone(),
                move |_, data, timing| Ok(video.packetize(data, timing)?.map(RtspPacket::Video)),
            )),
        ),
        UNSUPPORTED_BRANCH => {
            log::warn!("RTSP stream with encoding {encoding:?} is not bridged");
            (UNSUPPORTED_BRANCH, None)
        }
        audio => (
            audio,
            Some(appsink_callbacks(packet_sender.clone(), audio_packet)),
        ),
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
    packet_sender: TokioSender<RtspPacket>,
    mut to_packet: impl FnMut(&gst::Sample, &[u8], MediaTiming) -> Result<Option<RtspPacket>>
        + Send
        + 'static,
) -> AppSinkCallbacks {
    AppSinkCallbacks::builder()
        .new_sample(move |sink| {
            let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
            let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
            let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
            match to_packet(&sample, map.as_slice(), MediaTiming::of(buffer)) {
                Ok(Some(packet)) => packet_sender
                    .blocking_send(packet)
                    .map_err(|_| gst::FlowError::Eos)?,
                Ok(None) => {}
                Err(err) => log::warn!("RTSP sample dropped: {err:#}"),
            }
            Ok(gst::FlowSuccess::Ok)
        })
        .build()
}

fn audio_packet(
    sample: &gst::Sample,
    data: &[u8],
    timing: MediaTiming,
) -> Result<Option<RtspPacket>> {
    let Some(caps) = sample.caps() else {
        return Ok(None);
    };
    let format = AudioFormat::from_caps(caps)?;
    Ok(Some(RtspPacket::Audio(
        format.packet(data.to_vec(), timing),
    )))
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
    timestamp_us: u64,
    duration_us: Option<u64>,
    ingest_wallclock_micros: u64,
}

impl MediaTiming {
    fn of(buffer: &gst::BufferRef) -> Self {
        Self {
            timestamp_us: buffer.pts().map_or(0, |pts| pts.useconds()),
            duration_us: buffer.duration().map(|duration| duration.useconds()),
            ingest_wallclock_micros: now_micros(),
        }
    }
}

struct VideoPacketizer {
    payload_format: PayloadFormat,
    fallback_codec: String,
    parameter_sets: ParameterSetTracker,
    config: Option<AvcDecoderConfigurationRecord>,
}

impl VideoPacketizer {
    fn new(payload_format: PayloadFormat, fallback_codec: String) -> Self {
        Self {
            payload_format,
            fallback_codec,
            parameter_sets: ParameterSetTracker::new(),
            config: None,
        }
    }

    /// An Annex-B payload carries its parameter sets in band and no avcC, since a
    /// decoder given an avcC reads length prefixes (draft-ietf-moq-loc-01 §2.1).
    fn packetize(
        &mut self,
        access_unit: &[u8],
        timing: MediaTiming,
    ) -> Result<Option<EncodedPacket>> {
        let Some(unit) = self.parameter_sets.track(access_unit)? else {
            return Ok(None);
        };
        if let Some(config) = unit.config_changed {
            log::info!("RTSP video config: codec={}", config.codec_string());
            self.config = Some(config);
        }
        let (data, description) = match self.payload_format {
            PayloadFormat::AnnexB => (unit.data.to_vec(), None),
            PayloadFormat::Avcc => (
                annexb_to_avcc(&unit.data, NAL_LENGTH_SIZE).to_vec(),
                self.config
                    .as_ref()
                    .map(AvcDecoderConfigurationRecord::to_bytes),
            ),
        };
        let codec = self.config.as_ref().map_or_else(
            || self.fallback_codec.clone(),
            |config| config.codec_string(),
        );
        Ok(Some(EncodedPacket {
            data,
            is_keyframe: unit.is_keyframe,
            timestamp_us: timing.timestamp_us,
            ingest_wallclock_micros: timing.ingest_wallclock_micros,
            codec: unit.is_keyframe.then_some(codec),
            description_base64: description
                .filter(|_| unit.is_keyframe)
                .map(|bytes| general_purpose::STANDARD.encode(bytes)),
        }))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AudioFormat {
    codec: String,
    description_base64: Option<String>,
    sample_rate: Option<u32>,
    channels: Option<u8>,
}

impl AudioFormat {
    fn from_caps(caps: &gst::CapsRef) -> Result<Self> {
        let structure = caps.structure(0).context("audio caps are empty")?;
        let sample_rate = structure
            .get::<i32>("rate")
            .ok()
            .and_then(|rate| u32::try_from(rate).ok());
        let channels = structure
            .get::<i32>("channels")
            .ok()
            .and_then(|channels| u8::try_from(channels).ok());
        match structure.name().as_str() {
            name @ ("audio/x-alaw" | "audio/x-opus") => Ok(Self {
                codec: if name == "audio/x-alaw" {
                    "pcma"
                } else {
                    "opus"
                }
                .to_string(),
                description_base64: None,
                sample_rate,
                channels,
            }),
            "audio/mpeg" => {
                let codec_data = structure
                    .get::<gst::Buffer>("codec_data")
                    .context("AAC caps without codec_data")?;
                let codec_data = codec_data.map_readable()?;
                let config = AudioSpecificConfig::parse(codec_data.as_slice())?;
                Ok(Self {
                    codec: config.codec_string(),
                    description_base64: Some(
                        general_purpose::STANDARD.encode(codec_data.as_slice()),
                    ),
                    sample_rate: Some(config.sample_rate),
                    channels: Some(config.channel_count()),
                })
            }
            other => bail!("unsupported audio caps {other}"),
        }
    }

    fn packet(&self, data: Vec<u8>, timing: MediaTiming) -> EncodedAudioPacket {
        EncodedAudioPacket {
            data,
            timestamp_us: timing.timestamp_us,
            ingest_wallclock_micros: timing.ingest_wallclock_micros,
            duration_us: timing.duration_us,
            codec: self.codec.clone(),
            description_base64: self.description_base64.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
        }
    }
}

pub fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &str = "Z2QAMqw7UBIAUdCAAAADAIAAAA9C";
    const PPS: &str = "aO484QBCQgCEhARMUhuTxXyfk/k/J8nm5MkkLCJCkJyeT6/J/X5PrycmpMA=";
    const IDR_SLICE: [u8; 3] = [0x65, 0x88, 0x84];
    const NON_IDR_SLICE: [u8; 3] = [0x41, 0x9a, 0x02];
    const START_CODE: [u8; 4] = [0, 0, 0, 1];
    const TIMING: MediaTiming = MediaTiming {
        timestamp_us: 1_000,
        duration_us: Some(66_666),
        ingest_wallclock_micros: 2_000,
    };

    fn nal(base64: &str) -> Vec<u8> {
        general_purpose::STANDARD.decode(base64).unwrap()
    }

    fn annexb(nals: &[&[u8]]) -> Vec<u8> {
        nals.iter()
            .flat_map(|nal| START_CODE.iter().chain(nal.iter()).copied())
            .collect()
    }

    fn keyframe() -> Vec<u8> {
        annexb(&[&nal(SPS), &nal(PPS), &IDR_SLICE])
    }

    fn packetize(packetizer: &mut VideoPacketizer, access_unit: &[u8]) -> EncodedPacket {
        packetizer.packetize(access_unit, TIMING).unwrap().unwrap()
    }

    #[test]
    fn annexb_keyframe_carries_its_parameter_sets_and_no_avcc() {
        // Arrange
        let mut packetizer = VideoPacketizer::new(PayloadFormat::AnnexB, "avc1.640028".into());

        // Act
        let packet = packetize(&mut packetizer, &keyframe());

        // Assert
        assert_eq!(packet.data, keyframe());
        assert!(packet.is_keyframe);
        assert_eq!(packet.codec.as_deref(), Some("avc1.640032"));
        assert_eq!(packet.description_base64, None);
    }

    #[test]
    fn annexb_keyframe_without_parameter_sets_gets_the_last_ones() {
        // Arrange
        let mut packetizer = VideoPacketizer::new(PayloadFormat::AnnexB, "avc1.640028".into());
        packetize(&mut packetizer, &keyframe());

        // Act
        let packet = packetize(&mut packetizer, &annexb(&[&IDR_SLICE]));

        // Assert
        assert_eq!(packet.data, keyframe());
    }

    #[test]
    fn avcc_keyframe_is_length_prefixed_with_its_avcc() {
        // Arrange
        let mut packetizer = VideoPacketizer::new(PayloadFormat::Avcc, "avc1.640028".into());

        // Act
        let packet = packetize(&mut packetizer, &keyframe());

        // Assert
        let sps_length = u32::try_from(nal(SPS).len()).unwrap().to_be_bytes();
        assert_eq!(packet.data[..4], sps_length);
        let avcc = general_purpose::STANDARD
            .decode(packet.description_base64.unwrap())
            .unwrap();
        assert_eq!(avcc[..4], [1, 0x64, 0x00, 0x32]);
    }

    #[test]
    fn delta_frame_carries_neither_codec_nor_avcc() {
        // Arrange
        let mut packetizer = VideoPacketizer::new(PayloadFormat::Avcc, "avc1.640028".into());
        packetize(&mut packetizer, &keyframe());

        // Act
        let packet = packetize(&mut packetizer, &annexb(&[&NON_IDR_SLICE]));

        // Assert
        assert!(!packet.is_keyframe);
        assert_eq!(packet.codec, None);
        assert_eq!(packet.description_base64, None);
    }

    #[test]
    fn keyframe_before_any_parameter_set_falls_back_to_the_configured_codec() {
        // Arrange
        let mut packetizer = VideoPacketizer::new(PayloadFormat::AnnexB, "avc1.640028".into());

        // Act
        let packet = packetize(&mut packetizer, &annexb(&[&IDR_SLICE]));

        // Assert
        assert_eq!(packet.codec.as_deref(), Some("avc1.640028"));
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
        let format = AudioFormat::from_caps(&caps).unwrap();

        // Assert
        assert_eq!(
            format,
            AudioFormat {
                codec: "pcma".to_string(),
                description_base64: None,
                sample_rate: Some(8_000),
                channels: Some(1),
            }
        );
    }

    #[test]
    fn aac_caps_take_codec_and_config_from_codec_data() {
        // Arrange
        gst::init().unwrap();
        let config = AudioSpecificConfig::new(2, 48_000, 2).to_bytes();
        let caps = gst::Caps::builder("audio/mpeg")
            .field("mpegversion", 4i32)
            .field("codec_data", gst::Buffer::from_slice(config.clone()))
            .build();

        // Act
        let format = AudioFormat::from_caps(&caps).unwrap();

        // Assert
        assert_eq!(format.codec, "mp4a.40.2");
        assert_eq!(
            format.description_base64,
            Some(general_purpose::STANDARD.encode(&config))
        );
        assert_eq!(
            (format.sample_rate, format.channels),
            (Some(48_000), Some(2))
        );
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
