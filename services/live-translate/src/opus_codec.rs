use anyhow::{Context, Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks, AppSrc};
use tokio::sync::mpsc;

pub(crate) const GEMINI_INPUT_SAMPLE_RATE: u32 = 16_000;
pub(crate) const GEMINI_OUTPUT_SAMPLE_RATE: u32 = 24_000;
pub(crate) const TRANSLATED_BITRATE: u32 = 32_000;

pub(crate) struct OpusDecoder {
    pipeline: gst::Pipeline,
    source: AppSrc,
}

impl OpusDecoder {
    pub(crate) fn new(channels: u32, pcm_sender: mpsc::UnboundedSender<Vec<i16>>) -> Result<Self> {
        let description = format!(
            "appsrc name=source format=time is-live=true do-timestamp=true \
               caps=audio/x-opus,channel-mapping-family=(int)0,rate=(int)48000,channels=(int){channels} \
             ! opusdec ! audioconvert ! audioresample \
             ! audio/x-raw,format=S16LE,layout=interleaved,rate={GEMINI_INPUT_SAMPLE_RATE},channels=1 \
             ! appsink name=sink sync=false"
        );
        let (pipeline, source, sink) = build(&description)?;
        sink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let samples = pull_bytes(sink)?
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| i16::from_le_bytes(*pair))
                        .collect();
                    pcm_sender.send(samples).map_err(|_| gst::FlowError::Eos)?;
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        start(&pipeline)?;
        Ok(Self { pipeline, source })
    }

    pub(crate) fn push(&self, packet: &[u8]) -> Result<()> {
        push(&self.source, gst::Buffer::from_slice(packet.to_vec()))
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

pub(crate) struct OpusEncoder {
    pipeline: gst::Pipeline,
    source: AppSrc,
}

impl OpusEncoder {
    pub(crate) fn new(packet_sender: mpsc::UnboundedSender<Vec<u8>>) -> Result<Self> {
        let description = format!(
            "appsrc name=source format=time is-live=true do-timestamp=true \
               caps=audio/x-raw,format=S16LE,layout=interleaved,rate=(int){GEMINI_OUTPUT_SAMPLE_RATE},channels=(int)1 \
             ! opusenc frame-size=20 bitrate={TRANSLATED_BITRATE} audio-type=voice \
             ! appsink name=sink sync=false"
        );
        let (pipeline, source, sink) = build(&description)?;
        sink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    packet_sender
                        .send(pull_bytes(sink)?)
                        .map_err(|_| gst::FlowError::Eos)?;
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        start(&pipeline)?;
        Ok(Self { pipeline, source })
    }

    pub(crate) fn push(&self, frame: &[i16]) -> Result<()> {
        let bytes: Vec<u8> = frame
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        push(&self.source, gst::Buffer::from_slice(bytes))
    }
}

impl Drop for OpusEncoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn build(description: &str) -> Result<(gst::Pipeline, AppSrc, AppSink)> {
    gst::init()?;
    let pipeline = gst::parse::launch(description)
        .context("build Opus pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("Opus description did not produce a pipeline"))?;
    let source = pipeline
        .by_name("source")
        .context("missing appsrc")?
        .downcast::<AppSrc>()
        .map_err(|_| anyhow!("source is not an appsrc"))?;
    let sink = pipeline
        .by_name("sink")
        .context("missing appsink")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow!("sink is not an appsink"))?;
    Ok((pipeline, source, sink))
}

fn start(pipeline: &gst::Pipeline) -> Result<()> {
    pipeline
        .set_state(gst::State::Playing)
        .map(|_| ())
        .context("start Opus pipeline")
}

fn push(source: &AppSrc, buffer: gst::Buffer) -> Result<()> {
    source
        .push_buffer(buffer)
        .map(|_| ())
        .map_err(|error| anyhow!("push into Opus pipeline: {error:?}"))
}

fn pull_bytes(sink: &AppSink) -> Result<Vec<u8>, gst::FlowError> {
    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
    Ok(map.as_slice().to_vec())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const FRAMES: usize = 50;
    const SAMPLES_PER_FRAME: usize = 480;

    fn tone() -> Vec<i16> {
        (0..SAMPLES_PER_FRAME * FRAMES)
            .map(|index| ((index as f32 * 0.05).sin() * 8_000.0) as i16)
            .collect()
    }

    async fn receive_until<T>(
        receiver: &mut mpsc::UnboundedReceiver<T>,
        enough: impl Fn(&[T]) -> bool,
    ) -> Vec<T> {
        let mut received = Vec::new();
        while !enough(&received) {
            match tokio::time::timeout(Duration::from_secs(5), receiver.recv()).await {
                Ok(Some(item)) => received.push(item),
                _ => break,
            }
        }
        received
    }

    #[tokio::test]
    async fn each_twenty_millisecond_frame_becomes_one_packet() {
        // Arrange
        let (packet_sender, mut packet_receiver) = mpsc::unbounded_channel();
        let encoder = OpusEncoder::new(packet_sender).expect("encoder");

        // Act
        for frame in tone().as_chunks::<SAMPLES_PER_FRAME>().0 {
            encoder.push(frame).expect("pushed");
        }
        let packets =
            receive_until(&mut packet_receiver, |packets| packets.len() >= FRAMES - 1).await;

        // Assert: opusenc holds back its last frame until the next one arrives
        assert!(packets.len() >= FRAMES - 1, "got {} packets", packets.len());
    }

    #[tokio::test]
    async fn encoded_packets_decode_to_sixteen_kilohertz_mono() {
        // Arrange
        let (packet_sender, mut packet_receiver) = mpsc::unbounded_channel();
        let encoder = OpusEncoder::new(packet_sender).expect("encoder");
        for frame in tone().as_chunks::<SAMPLES_PER_FRAME>().0 {
            encoder.push(frame).expect("pushed");
        }
        let packets =
            receive_until(&mut packet_receiver, |packets| packets.len() >= FRAMES - 1).await;
        let (pcm_sender, mut pcm_receiver) = mpsc::unbounded_channel();
        let decoder = OpusDecoder::new(1, pcm_sender).expect("decoder");

        // Act
        for packet in &packets {
            decoder.push(packet).expect("pushed");
        }
        let one_second = GEMINI_INPUT_SAMPLE_RATE as usize * packets.len() / FRAMES;
        let pcm = receive_until(&mut pcm_receiver, |chunks| {
            chunks.iter().map(Vec::len).sum::<usize>() >= one_second * 9 / 10
        })
        .await;

        // Assert: one second of 24 kHz input comes back as roughly 16 000 samples
        let samples: usize = pcm.iter().map(Vec::len).sum();
        assert!(samples >= one_second * 9 / 10, "got {samples} samples");
        assert!(
            pcm.iter()
                .flatten()
                .any(|&sample| sample.unsigned_abs() > 1_000)
        );
    }
}
