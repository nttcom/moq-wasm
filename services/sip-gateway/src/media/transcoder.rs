use anyhow::{Context, Result, anyhow};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks, AppSrc};
use tokio::sync::mpsc;

use crate::media::g711::G711;

pub(crate) const OPUS_SAMPLE_RATE: u32 = 48_000;
/// Enough for narrowband speech; G.711 carries nothing above 4 kHz.
const OPUS_BITRATE: u32 = 24_000;

/// One GStreamer pipeline from an `appsrc` to an `appsink`, each buffer the
/// sink pulls sent as bytes.
pub(crate) struct Transcoder {
    pipeline: gst::Pipeline,
    source: AppSrc,
}

impl Transcoder {
    /// G.711 from the phone to one Opus packet per 20 ms.
    pub(crate) fn g711_to_opus(
        codec: G711,
        packet_sender: mpsc::UnboundedSender<Vec<u8>>,
    ) -> Result<Self> {
        Self::run(
            &format!(
                "appsrc name=source format=time is-live=true do-timestamp=true caps={caps} \
                 ! {decoder} ! audioconvert ! audioresample \
                 ! audio/x-raw,rate={OPUS_SAMPLE_RATE},channels=1 \
                 ! opusenc frame-size=20 bitrate={OPUS_BITRATE} audio-type=voice \
                 ! appsink name=sink sync=false",
                caps = codec.caps(),
                decoder = codec.decoder(),
            ),
            packet_sender,
        )
    }

    /// Opus packets from the peer to G.711 bytes in whatever lengths the
    /// encoder hands out.
    pub(crate) fn opus_to_g711(
        codec: G711,
        channels: u8,
        g711_sender: mpsc::UnboundedSender<Vec<u8>>,
    ) -> Result<Self> {
        Self::run(
            &format!(
                "appsrc name=source format=time is-live=true do-timestamp=true \
                   caps=audio/x-opus,channel-mapping-family=(int)0,rate=(int){OPUS_SAMPLE_RATE},channels=(int){channels} \
                 ! opusdec ! audioconvert ! audioresample \
                 ! audio/x-raw,format=S16LE,rate=8000,channels=1 \
                 ! {encoder} ! appsink name=sink sync=false",
                encoder = codec.encoder(),
            ),
            g711_sender,
        )
    }

    fn run(description: &str, output_sender: mpsc::UnboundedSender<Vec<u8>>) -> Result<Self> {
        gst::init()?;
        let pipeline = gst::parse::launch(description)
            .context("build transcoding pipeline")?
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow!("the description did not produce a pipeline"))?;
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
        sink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    output_sender
                        .send(pull_bytes(sink)?)
                        .map_err(|_| gst::FlowError::Eos)?;
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        pipeline
            .set_state(gst::State::Playing)
            .context("start transcoding pipeline")?;
        Ok(Self { pipeline, source })
    }

    pub(crate) fn push(&self, data: &[u8]) -> Result<()> {
        self.source
            .push_buffer(gst::Buffer::from_slice(data.to_vec()))
            .map(|_| ())
            .map_err(|error| anyhow!("push into transcoding pipeline: {error:?}"))
    }
}

impl Drop for Transcoder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
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
    use crate::media::g711::SAMPLES_PER_PACKET;

    const PACKETS: usize = 50;

    async fn receive_at_least(
        receiver: &mut mpsc::UnboundedReceiver<Vec<u8>>,
        bytes: usize,
    ) -> Vec<Vec<u8>> {
        let mut received = Vec::new();
        while received.iter().map(Vec::len).sum::<usize>() < bytes {
            match tokio::time::timeout(Duration::from_secs(5), receiver.recv()).await {
                Ok(Some(chunk)) => received.push(chunk),
                _ => break,
            }
        }
        received
    }

    #[tokio::test]
    async fn a_second_of_g711_becomes_opus_and_back() {
        // Arrange: a 500 Hz square wave in μ-law
        let tone: Vec<u8> = (0..SAMPLES_PER_PACKET * PACKETS)
            .map(|index| if (index / 8) % 2 == 0 { 0x20 } else { 0xA0 })
            .collect();
        let (opus_sender, mut opus_receiver) = mpsc::unbounded_channel();
        let to_opus = Transcoder::g711_to_opus(G711::Pcmu, opus_sender).unwrap();
        let (g711_sender, mut g711_receiver) = mpsc::unbounded_channel();
        let to_g711 = Transcoder::opus_to_g711(G711::Pcmu, 1, g711_sender).unwrap();

        // Act
        for packet in tone.chunks(SAMPLES_PER_PACKET) {
            to_opus.push(packet).unwrap();
        }
        let mut opus = Vec::new();
        while opus.len() < PACKETS - 2 {
            match tokio::time::timeout(Duration::from_secs(5), opus_receiver.recv()).await {
                Ok(Some(packet)) => opus.push(packet),
                _ => break,
            }
        }
        for packet in &opus {
            to_g711.push(packet).unwrap();
        }
        let g711 = receive_at_least(&mut g711_receiver, tone.len() * 9 / 10).await;

        // Assert: opusenc holds back its last frames until more audio arrives
        assert!(opus.len() >= PACKETS - 2, "got {} Opus packets", opus.len());
        let samples: usize = g711.iter().map(Vec::len).sum();
        assert!(samples >= tone.len() * 9 / 10, "got {samples} samples");
        assert!(
            g711.iter()
                .flatten()
                .any(|&byte| byte != G711::Pcmu.silence())
        );
    }
}
