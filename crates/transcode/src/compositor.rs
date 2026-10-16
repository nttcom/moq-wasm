use std::{path::Path, thread::JoinHandle};

use anyhow::{Context, Result, anyhow, bail, ensure};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::AppSrc;
use mediapack::{AudioConfig, MediaEvent, VideoConfig, VideoSample};
use tokio::sync::mpsc;

use crate::buffer::video_buffer;

mod layout;

use layout::FrameSpans;

/// Yields a `VideoConfig` or `AudioConfig` first, then samples whose
/// timestamps are already on the shared timeline of the composition. A later
/// config event applies to the samples after it.
pub type EventSource = Box<dyn Iterator<Item = Result<MediaEvent>> + Send>;

const INPUT_QUEUE_BYTES: u64 = 4 * 1024 * 1024;
/// A frame followed by a longer gap (a camera turned off) is shown this
/// long; the participant then leaves the grid until its next frame.
const MAX_FRAME_DURATION: gst::ClockTime = gst::ClockTime::SECOND;
const OUTPUT_AUDIO_CAPS: &str = "audio/x-raw,format=F32LE,layout=interleaved,rate=48000,channels=2";

/// One participant. An input without video only feeds the audio mix and
/// takes no tile.
pub struct CompositeInput {
    pub video: Option<EventSource>,
    pub audio: Option<EventSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompositeSettings {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
}

impl Default for CompositeSettings {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            framerate: 15,
            video_bitrate_kbps: 2_000,
            audio_bitrate_kbps: 128,
        }
    }
}

enum Message {
    Error(anyhow::Error),
    EndOfStream,
}

/// Composites the inputs offline into one progressive MP4: the videos that
/// have a frame at each moment as a grid of equal tiles on a black background
/// (H.264), the audio mixed into one stereo track (AAC). Dropping the
/// compositor stops the pipeline.
///
/// A later config event may change the resolution or the codec: decodebin3
/// follows the new parameter sets or caps, and the picture rescales to its
/// tile.
pub struct Compositor {
    pipeline: gst::Pipeline,
    message_receiver: mpsc::UnboundedReceiver<Message>,
    _feeders: Vec<JoinHandle<()>>,
}

impl Compositor {
    /// Fails when no input carries media or the settings cannot be encoded.
    pub fn start(
        mut inputs: Vec<CompositeInput>,
        settings: CompositeSettings,
        output: &Path,
    ) -> Result<Self> {
        ensure!(
            settings.width >= 2 && settings.height >= 2 && settings.framerate > 0,
            "invalid composite settings {settings:?}"
        );
        let video_sources: Vec<EventSource> = inputs
            .iter_mut()
            .filter_map(|input| input.video.take())
            .collect();
        let audio_sources: Vec<EventSource> =
            inputs.into_iter().filter_map(|input| input.audio).collect();
        ensure!(
            !video_sources.is_empty() || !audio_sources.is_empty(),
            "no input carries media"
        );
        gst::init()?;
        let pipeline = gst::parse::launch(&pipeline_description(
            video_sources.len(),
            audio_sources.len(),
            &settings,
        ))
        .context("build composite pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("composite description did not produce a pipeline"))?;
        pipeline
            .by_name("sink")
            .context("missing filesink")?
            .set_property(
                "location",
                output.to_str().context("output path is not UTF-8")?,
            );
        let frames: Vec<FrameSpans> = video_sources
            .iter()
            .map(|_| FrameSpans::default())
            .collect();
        if !frames.is_empty() {
            let compositor = pipeline.by_name("comp").context("missing compositor")?;
            layout::install(&compositor, frames.clone(), settings)?;
        }
        let (message_sender, message_receiver) = mpsc::unbounded_channel();
        forward_bus_messages(&pipeline, message_sender.clone())?;

        let last_frame_duration = gst::ClockTime::SECOND / u64::from(settings.framerate);
        let mut feeders = Vec::new();
        for (index, (events, frames)) in video_sources.into_iter().zip(frames).enumerate() {
            let source = app_source(&pipeline, &format!("video{index}"))?;
            let message_sender = message_sender.clone();
            feeders.push(spawn_feeder(
                format!("video{index}"),
                message_sender,
                move || feed_video(&source, events, &frames, last_frame_duration),
            )?);
        }
        for (index, events) in audio_sources.into_iter().enumerate() {
            let source = app_source(&pipeline, &format!("audio{index}"))?;
            let message_sender = message_sender.clone();
            feeders.push(spawn_feeder(
                format!("audio{index}"),
                message_sender,
                move || feed_audio(&source, events),
            )?);
        }
        pipeline
            .set_state(gst::State::Playing)
            .context("start composite pipeline")?;
        Ok(Self {
            pipeline,
            message_receiver,
            _feeders: feeders,
        })
    }

    /// Resolves once the output file is complete, or with the first pipeline
    /// or input error.
    pub async fn finished(&mut self) -> Result<()> {
        match self.message_receiver.recv().await {
            Some(Message::EndOfStream) => {
                self.pipeline
                    .set_state(gst::State::Null)
                    .context("close composite output")?;
                Ok(())
            }
            Some(Message::Error(error)) => Err(error),
            None => bail!("composite pipeline stopped reporting"),
        }
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn pipeline_description(
    video_count: usize,
    audio_count: usize,
    settings: &CompositeSettings,
) -> String {
    let source = format!("format=time block=true max-bytes={INPUT_QUEUE_BYTES}");
    let mut description = String::from("mp4mux name=mux faststart=true ! filesink name=sink ");
    if video_count > 0 {
        description.push_str("compositor name=comp background=black ");
        for index in 0..video_count {
            description.push_str(&format!("sink_{index}::sizing-policy=keep-aspect-ratio "));
        }
        description.push_str(&format!(
            "! video/x-raw,format=I420,width={},height={},framerate={}/1 ! \
             x264enc speed-preset=veryfast bitrate={} ! h264parse ! queue ! mux. ",
            settings.width, settings.height, settings.framerate, settings.video_bitrate_kbps
        ));
        for index in 0..video_count {
            description.push_str(&format!(
                "appsrc name=video{index} {source} ! decodebin3 ! videoconvert ! comp.sink_{index} "
            ));
        }
    }
    if audio_count > 0 {
        description.push_str(&format!(
            "audiomixer name=mix ! {OUTPUT_AUDIO_CAPS} ! audioconvert ! audioresample ! \
             avenc_aac bitrate={} ! aacparse ! queue ! mux. ",
            u64::from(settings.audio_bitrate_kbps) * 1_000
        ));
        for index in 0..audio_count {
            description.push_str(&format!(
                "appsrc name=audio{index} {source} ! decodebin3 ! audioconvert ! audioresample ! mix. "
            ));
        }
    }
    description
}

fn app_source(pipeline: &gst::Pipeline, name: &str) -> Result<AppSrc> {
    pipeline
        .by_name(name)
        .with_context(|| format!("missing appsrc {name}"))?
        .downcast::<AppSrc>()
        .map_err(|_| anyhow!("{name} is not an appsrc"))
}

/// Each source gets its own thread: compositor and audiomixer wait for a
/// buffer on every pad, so one thread interleaving two sources blocks on the
/// first full queue while the mixer waits for the other.
fn spawn_feeder(
    name: String,
    message_sender: mpsc::UnboundedSender<Message>,
    feed: impl FnOnce() -> Result<()> + Send + 'static,
) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("composite-{name}"))
        .spawn(move || {
            if let Err(error) = feed() {
                let _ = message_sender.send(Message::Error(error.context(format!("feed {name}"))));
            }
        })
        .context("spawn composite feeder")
}

/// A frame lasts until the next frame's decode time: decode-time spacing
/// equals presentation-time spacing, also with B-frames.
fn feed_video(
    source: &AppSrc,
    events: EventSource,
    frames: &FrameSpans,
    last_frame_duration: gst::ClockTime,
) -> Result<()> {
    let mut held: Option<VideoSample> = None;
    // A config applies after the held frame, which is pushed only once the
    // next frame gives its duration.
    let mut next_caps = None;
    for event in events {
        match event? {
            MediaEvent::VideoConfig(config) => next_caps = Some(video_caps(&config)),
            MediaEvent::Video(sample) => {
                if let Some(previous) = held.take() {
                    let duration = gst::ClockTime::from_useconds(
                        sample.dts.saturating_sub(previous.dts).micros(),
                    )
                    .min(MAX_FRAME_DURATION);
                    push_frame(source, frames, &previous, duration)?;
                }
                if let Some(caps) = next_caps.take() {
                    source.set_caps(Some(&caps));
                }
                ensure!(source.caps().is_some(), "video sample before a VideoConfig");
                held = Some(sample);
            }
            MediaEvent::Streams(_) => {}
            MediaEvent::AudioConfig(_) | MediaEvent::Audio(_) => bail!("audio on a video source"),
        }
    }
    if let Some(last) = held {
        push_frame(source, frames, &last, last_frame_duration)?;
    }
    end_of_stream(source)
}

fn push_frame(
    source: &AppSrc,
    frames: &FrameSpans,
    sample: &VideoSample,
    duration: gst::ClockTime,
) -> Result<()> {
    let pts = gst::ClockTime::from_useconds(sample.pts.micros());
    frames.record(pts, pts + duration);
    push(source, video_buffer(sample, Some(duration))?)
}

fn feed_audio(source: &AppSrc, events: EventSource) -> Result<()> {
    for event in events {
        match event? {
            MediaEvent::AudioConfig(config) => source.set_caps(Some(&audio_caps(&config))),
            MediaEvent::Audio(sample) => {
                ensure!(
                    source.caps().is_some(),
                    "audio sample before an AudioConfig"
                );
                let mut buffer = gst::Buffer::from_slice(sample.data);
                buffer
                    .get_mut()
                    .ok_or_else(|| anyhow!("fresh buffer is shared"))?
                    .set_pts(gst::ClockTime::from_useconds(sample.pts.micros()));
                push(source, buffer)?;
            }
            MediaEvent::Streams(_) => {}
            MediaEvent::VideoConfig(_) | MediaEvent::Video(_) => bail!("video on an audio source"),
        }
    }
    end_of_stream(source)
}

fn push(source: &AppSrc, buffer: gst::Buffer) -> Result<()> {
    source
        .push_buffer(buffer)
        .map(|_| ())
        .map_err(|error| anyhow!("push sample into compositor: {error:?}"))
}

fn end_of_stream(source: &AppSrc) -> Result<()> {
    source
        .end_of_stream()
        .map(|_| ())
        .map_err(|error| anyhow!("signal end of stream: {error:?}"))
}

/// H.264 samples are Annex-B with in-band parameter sets, so a resolution
/// change keeps the same caps.
fn video_caps(config: &VideoConfig) -> gst::Caps {
    match config {
        VideoConfig::Avc(_) => gst::Caps::builder("video/x-h264")
            .field("stream-format", "byte-stream")
            .field("alignment", "au")
            .build(),
        VideoConfig::Vp8(_) => gst::Caps::new_empty_simple("video/x-vp8"),
        VideoConfig::Vp9(_) => gst::Caps::new_empty_simple("video/x-vp9"),
        VideoConfig::Av1(_) => gst::Caps::builder("video/x-av1")
            .field("stream-format", "obu-stream")
            .field("alignment", "tu")
            .build(),
    }
}

fn audio_caps(config: &AudioConfig) -> gst::Caps {
    let caps = match config {
        AudioConfig::Aac(config) => gst::Caps::builder("audio/mpeg")
            .field("mpegversion", 4i32)
            .field("stream-format", "raw")
            .field("codec_data", gst::Buffer::from_slice(config.to_bytes())),
        AudioConfig::Opus { .. } => {
            gst::Caps::builder("audio/x-opus").field("channel-mapping-family", 0i32)
        }
        AudioConfig::Pcma { .. } => gst::Caps::builder("audio/x-alaw"),
        AudioConfig::Mp3 { .. } => gst::Caps::builder("audio/mpeg")
            .field("mpegversion", 1i32)
            .field("layer", 3i32),
    };
    caps.field("rate", config.sample_rate() as i32)
        .field("channels", i32::from(config.channels()))
        .build()
}

fn forward_bus_messages(
    pipeline: &gst::Pipeline,
    message_sender: mpsc::UnboundedSender<Message>,
) -> Result<()> {
    let bus = pipeline.bus().context("composite pipeline has no bus")?;
    bus.set_sync_handler(move |_, message| {
        match message.view() {
            gst::MessageView::Error(error) => {
                let _ = message_sender.send(Message::Error(anyhow!(
                    "composite pipeline error: {} ({:?})",
                    error.error(),
                    error.debug()
                )));
            }
            gst::MessageView::Eos(_) => {
                let _ = message_sender.send(Message::EndOfStream);
            }
            _ => {}
        }
        gst::BusSyncReply::Drop
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use mediapack::{Timestamp, mp4::Mp4Index};

    use super::*;
    use crate::{
        Rendition, Transcoder,
        test_support::{
            FIXTURE_AV1_IVF, FIXTURE_VP8_IVF, FIXTURE_VP9_IVF, decode_in_software, fixture_events,
            ivf_frames,
        },
    };

    const SETTINGS: CompositeSettings = CompositeSettings {
        width: 320,
        height: 180,
        framerate: 15,
        video_bitrate_kbps: 200,
        audio_bitrate_kbps: 64,
    };
    const TIMEOUT: Duration = Duration::from_secs(60);

    fn source(events: Vec<MediaEvent>) -> Option<EventSource> {
        Some(Box::new(events.into_iter().map(Ok)))
    }

    fn video_events() -> Vec<MediaEvent> {
        fixture_events()
            .into_iter()
            .filter(|event| matches!(event, MediaEvent::VideoConfig(_) | MediaEvent::Video(_)))
            .collect()
    }

    fn audio_events() -> Vec<MediaEvent> {
        fixture_events()
            .into_iter()
            .filter(|event| matches!(event, MediaEvent::AudioConfig(_) | MediaEvent::Audio(_)))
            .collect()
    }

    fn shifted(events: Vec<MediaEvent>, shift: Timestamp) -> Vec<MediaEvent> {
        events
            .into_iter()
            .map(|event| match event {
                MediaEvent::Video(mut sample) => {
                    sample.pts = sample.pts.saturating_add(shift);
                    sample.dts = sample.dts.saturating_add(shift);
                    MediaEvent::Video(sample)
                }
                MediaEvent::Audio(mut sample) => {
                    sample.pts = sample.pts.saturating_add(shift);
                    MediaEvent::Audio(sample)
                }
                other => other,
            })
            .collect()
    }

    /// Where the last video frame of `events` ends, given the last frame
    /// lasts one output frame.
    fn video_end(events: &[MediaEvent]) -> Timestamp {
        let last = events
            .iter()
            .filter_map(|event| match event {
                MediaEvent::Video(sample) => Some(sample.pts),
                _ => None,
            })
            .max()
            .unwrap();
        last.saturating_add(Timestamp::from_micros(
            1_000_000 / u64::from(SETTINGS.framerate),
        ))
    }

    fn ivf_events(
        file: &[u8],
        parse: impl Fn(&Bytes) -> (bool, Option<VideoConfig>),
    ) -> Vec<MediaEvent> {
        let mut events = Vec::new();
        for (index, data) in ivf_frames(file).into_iter().enumerate() {
            let (is_keyframe, config) = parse(&data);
            if let Some(config) = config {
                events.push(MediaEvent::VideoConfig(config));
            }
            let pts = Timestamp::from_ticks(index as u64, 30);
            events.push(MediaEvent::Video(VideoSample {
                data,
                is_keyframe,
                pts,
                dts: pts,
            }));
        }
        events
    }

    async fn composite(inputs: Vec<CompositeInput>, output: &Path) -> Result<()> {
        let mut compositor = Compositor::start(inputs, SETTINGS, output)?;
        tokio::time::timeout(TIMEOUT, compositor.finished())
            .await
            .context("compositor did not finish")?
    }

    fn read_index(path: &Path) -> Mp4Index {
        let file = std::fs::read(path).unwrap();
        let mut position = 0;
        while position + 8 <= file.len() {
            let size =
                u32::from_be_bytes(file[position..position + 4].try_into().unwrap()) as usize;
            if &file[position + 4..position + 8] == b"moov" {
                return Mp4Index::parse(&file[position..position + size]).unwrap();
            }
            position += size;
        }
        panic!("{} has no moov", path.display());
    }

    fn output_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "transcode-compositor-{}-{name}.mp4",
            std::process::id()
        ))
    }

    fn assert_duration(index: &Mp4Index, expected: Timestamp) {
        let duration = index.duration().micros() as i64;
        let expected = expected.micros() as i64;
        assert!(
            (duration - expected).abs() < 150_000,
            "duration {duration} µs, expected about {expected} µs"
        );
    }

    #[tokio::test]
    async fn composites_two_participants_into_one_mp4() {
        // Arrange: the second participant joins 2 s later
        decode_in_software();
        let shift = Timestamp::from_millis(2_000);
        let later_video = shifted(video_events(), shift);
        let expected_end = video_end(&later_video);
        let inputs = vec![
            CompositeInput {
                video: source(video_events()),
                audio: source(audio_events()),
            },
            CompositeInput {
                video: source(later_video),
                audio: source(shifted(audio_events(), shift)),
            },
        ];
        let output = output_path("two");

        // Act
        composite(inputs, &output).await.unwrap();

        // Assert
        let index = read_index(&output);
        let sps = index
            .video
            .as_ref()
            .unwrap()
            .sequence_parameter_set()
            .unwrap();
        assert_eq!((sps.width, sps.height), (SETTINGS.width, SETTINGS.height));
        assert!(matches!(index.audio, Some(AudioConfig::Aac(_))));
        assert_duration(&index, expected_end);
        std::fs::remove_file(output).unwrap();
    }

    #[tokio::test]
    async fn finishes_across_a_gap_in_the_video() {
        // Arrange: the camera is off for 3 s between two runs of the fixture
        decode_in_software();
        let first = video_events();
        let shift = video_end(&first).saturating_add(Timestamp::from_millis(3_000));
        let second = shifted(
            video_events(),
            shift.saturating_sub(Timestamp::from_micros(1_421_333)),
        );
        let expected_end = video_end(&second);
        let events = first.into_iter().chain(second).collect();
        let inputs = vec![CompositeInput {
            video: source(events),
            audio: None,
        }];
        let output = output_path("gap");

        // Act
        composite(inputs, &output).await.unwrap();

        // Assert
        let index = read_index(&output);
        assert!(index.audio.is_none());
        assert_duration(&index, expected_end);
        std::fs::remove_file(output).unwrap();
    }

    #[tokio::test]
    async fn mixes_an_audio_only_participant() {
        // Arrange
        decode_in_software();
        let inputs = vec![
            CompositeInput {
                video: None,
                audio: source(audio_events()),
            },
            CompositeInput {
                video: source(video_events()),
                audio: None,
            },
        ];
        let output = output_path("audio-only");

        // Act
        composite(inputs, &output).await.unwrap();

        // Assert
        let index = read_index(&output);
        let sps = index
            .video
            .as_ref()
            .unwrap()
            .sequence_parameter_set()
            .unwrap();
        assert_eq!((sps.width, sps.height), (SETTINGS.width, SETTINGS.height));
        assert!(matches!(index.audio, Some(AudioConfig::Aac(_))));
        std::fs::remove_file(output).unwrap();
    }

    #[tokio::test]
    async fn decodes_vp8_vp9_and_av1_inputs() {
        // Arrange
        let vp8 = ivf_events(FIXTURE_VP8_IVF, |data| {
            let frame = mediapack::vp8::parse_frame(data).unwrap();
            (frame.is_keyframe, frame.config.map(VideoConfig::Vp8))
        });
        let vp9 = ivf_events(FIXTURE_VP9_IVF, |data| {
            let frame = mediapack::vp9::parse_frame(data).unwrap();
            (frame.is_keyframe, frame.config.map(VideoConfig::Vp9))
        });
        let av1_config = std::sync::Mutex::new(None);
        let av1 = ivf_events(FIXTURE_AV1_IVF, |data| {
            let mut current = av1_config.lock().unwrap();
            let unit = mediapack::av1::parse_temporal_unit(data, current.as_ref()).unwrap();
            if let Some(config) = &unit.config {
                *current = Some(config.clone());
            }
            (unit.is_keyframe, unit.config.map(VideoConfig::Av1))
        });
        let inputs = [vp8, vp9, av1]
            .into_iter()
            .map(|events| CompositeInput {
                video: source(events),
                audio: None,
            })
            .collect();
        let output = output_path("codecs");

        // Act
        composite(inputs, &output).await.unwrap();

        // Assert
        let index = read_index(&output);
        let sps = index
            .video
            .as_ref()
            .unwrap()
            .sequence_parameter_set()
            .unwrap();
        assert_eq!((sps.width, sps.height), (SETTINGS.width, SETTINGS.height));
        std::fs::remove_file(output).unwrap();
    }

    #[tokio::test]
    async fn follows_a_resolution_change_within_one_input() {
        // Arrange: the fixture at 160x90, then the same pictures re-encoded at 96x54
        decode_in_software();
        let first = video_events();
        let mut transcoder = Transcoder::new(&[Rendition {
            name: "54p".into(),
            width: 96,
            height: 54,
            bitrate_kbps: 100,
        }])
        .unwrap();
        for event in &first {
            if let MediaEvent::Video(sample) = event {
                transcoder.push(sample).unwrap();
            }
        }
        transcoder.finish().unwrap();
        let mut smaller = Vec::new();
        while let Some(output) = transcoder.next().await {
            smaller.push(output.unwrap().event);
        }
        let smaller = shifted(
            smaller,
            video_end(&first).saturating_sub(Timestamp::from_micros(1_421_333)),
        );
        let expected_end = video_end(&smaller);
        let inputs = vec![CompositeInput {
            video: source(first.into_iter().chain(smaller).collect()),
            audio: None,
        }];
        let output = output_path("resize");

        // Act
        composite(inputs, &output).await.unwrap();

        // Assert
        assert_duration(&read_index(&output), expected_end);
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn rejects_inputs_without_media() {
        // Arrange
        let inputs = vec![CompositeInput {
            video: None,
            audio: None,
        }];

        // Act / Assert
        assert!(Compositor::start(inputs, SETTINGS, &output_path("empty")).is_err());
    }
}
