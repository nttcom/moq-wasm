use bytes::Bytes;
use gstreamer::{
    glib::object::Cast,
    prelude::{ElementExt, GstBinExt},
};

use crate::video_sender::SendData;

pub(crate) struct AudioSender {
    pipeline: gstreamer::Bin,
    _appsink: gstreamer_app::AppSink,
}

impl AudioSender {
    pub(crate) async fn new(data_sender: tokio::sync::mpsc::Sender<SendData>) -> Self {
        gstreamer::init().unwrap();
        let pipeline_str = "osxaudiosrc ! audioconvert ! audioresample ! \
                                audio/x-raw,rate=48000,channels=1 ! \
                                opusenc bitrate=32000 audio-type=voice frame-size=20 ! \
                                appsink name=audio_sink sync=false max-buffers=1 drop=true";
        // if you stream video file you want, comment out below and designate absolute path.
        // let pipeline_str = "filesrc location=path/to/something.mp4 ! decodebin ! videoconvert ! x264enc key-int-max=30 ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream ! appsink name=sink";
        let pipeline = gstreamer::parse::launch(pipeline_str).unwrap();
        let pipeline = pipeline.downcast::<gstreamer::Bin>().unwrap();
        let audio_appsink = pipeline
            .by_name("audio_sink")
            .unwrap()
            .dynamic_cast::<gstreamer_app::AppSink>()
            .unwrap();
        Self::set_audio_appsink_callbacks(&audio_appsink, data_sender);

        Self {
            pipeline,
            _appsink: audio_appsink,
        }
    }

    fn set_audio_appsink_callbacks(
        appsink: &gstreamer_app::AppSink,
        sender: tokio::sync::mpsc::Sender<SendData>,
    ) {
        let group_id = 0;
        let mut object_id = 0;
        appsink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    // tracing::info!("New sample received from appsink");
                    let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gstreamer::FlowError::Error)?;

                    object_id += 1;

                    let map = buffer
                        .map_readable()
                        .map_err(|_| gstreamer::FlowError::Error)?;

                    let send_data = SendData {
                        group_id,
                        object_id,
                        payload: Bytes::copy_from_slice(map.as_slice()),
                    };
                    if let Err(e) = sender.blocking_send(send_data) {
                        tracing::error!("Failed to enqueue audio sample: {}", e);
                        return Err(gstreamer::FlowError::Eos);
                    }
                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );
    }

    pub(crate) fn start(&self) {
        self.pipeline.set_state(gstreamer::State::Playing).unwrap();
    }
}
