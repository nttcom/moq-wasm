use bytes::Bytes;
use gstreamer::{
    glib::object::Cast,
    prelude::{ElementExt, GstBinExt},
};

pub(super) struct SendData {
    pub group_id: u64,
    pub object_id: u64,
    pub payload: Bytes,
}

pub(crate) struct GStreamerSender {
    pipeline: gstreamer::Bin,
    _appsink: gstreamer_app::AppSink,
}

impl GStreamerSender {
    pub(crate) async fn new(data_sender: tokio::sync::mpsc::Sender<SendData>) -> Self {
        gstreamer::init().unwrap();
        let pipeline_str = "videotestsrc ! videoconvert ! x264enc key-int-max=30 ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream ! appsink name=sink";
        let pipeline = gstreamer::parse::launch(pipeline_str).unwrap();
        let pipeline = pipeline.downcast::<gstreamer::Bin>().unwrap();
        let appsink = pipeline
            .by_name("sink")
            .unwrap()
            .dynamic_cast::<gstreamer_app::AppSink>()
            .unwrap();
        Self::set_appsink_callbacks(&appsink, data_sender);

        Self {
            pipeline,
            _appsink: appsink,
        }
    }

    fn set_appsink_callbacks(
        appsink: &gstreamer_app::AppSink,
        sender: tokio::sync::mpsc::Sender<SendData>,
    ) {
        let mut group_id = 0;
        let mut object_id = 0;
        appsink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    tracing::info!("New sample received from appsink");
                    let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gstreamer::FlowError::Error)?;
                    let is_keyframe = !buffer.flags().contains(gstreamer::BufferFlags::DELTA_UNIT);

                    if is_keyframe {
                        println!("🔑 key frame");
                        group_id += 1;
                        object_id = 0;
                    } else {
                        println!("📹 delta frame (P/B frame)");
                        object_id += 1;
                    }

                    let map = buffer
                        .map_readable()
                        .map_err(|_| gstreamer::FlowError::Error)?;

                    let send_data = SendData {
                        group_id,
                        object_id,
                        payload: Bytes::copy_from_slice(map.as_slice()),
                    };
                    let _ = sender.try_send(send_data);
                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );
    }

    pub(crate) fn start(&self) {
        self.pipeline.set_state(gstreamer::State::Playing).unwrap();
    }
}
