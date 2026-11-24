use std::{sync::Arc, time::Duration};

use broadcast::BroadcastWriter;
use bytes::Bytes;
use mediapack::{AudioConfig, AudioSample, MediaEvent, Timestamp};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
    time::Instant,
};

use crate::media::{
    g711::G711,
    rtp_leg::RtpLeg,
    transcoder::{OPUS_SAMPLE_RATE, Transcoder},
};

const OPUS_FRAME: Duration = Duration::from_millis(20);

/// Publishes the trunk's audio as the broadcast's Opus track and reports
/// when it last arrived, so a call whose trunk went silent can end.
pub(crate) struct PhoneAudioTask {
    join_handle: JoinHandle<()>,
}

impl PhoneAudioTask {
    pub(crate) fn run(
        leg: Arc<RtpLeg>,
        codec: G711,
        mut writer: BroadcastWriter,
        last_audio_sender: watch::Sender<Instant>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let (opus_sender, mut opus_receiver) = mpsc::unbounded_channel();
            let transcoder = match Transcoder::g711_to_opus(codec, opus_sender) {
                Ok(transcoder) => transcoder,
                Err(error) => {
                    tracing::error!(%error, "phone audio not transcoded");
                    return;
                }
            };
            let receive = leg.receive(codec, |payload| {
                last_audio_sender.send_replace(Instant::now());
                if let Err(error) = transcoder.push(payload) {
                    tracing::warn!(%error, "phone audio dropped");
                }
            });
            let publish = async {
                if let Err(error) = writer.push(MediaEvent::AudioConfig(AudioConfig::Opus {
                    sample_rate: OPUS_SAMPLE_RATE,
                    channels: 1,
                })) {
                    tracing::error!(%error, "phone audio not published");
                    return;
                }
                // Every Opus frame is 20 ms, so the frames' count is the
                // audio's timeline.
                let mut frames: u64 = 0;
                while let Some(packet) = opus_receiver.recv().await {
                    let pts = Timestamp::from_micros(frames * OPUS_FRAME.as_micros() as u64);
                    frames += 1;
                    if let Err(error) = writer.push(MediaEvent::Audio(AudioSample {
                        data: Bytes::from(packet),
                        pts,
                        audio_level: None,
                    })) {
                        tracing::error!(%error, "phone audio not published");
                        return;
                    }
                }
            };
            tokio::select! {
                () = receive => {}
                () = publish => {}
            }
        });
        Self { join_handle }
    }
}

impl Drop for PhoneAudioTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
