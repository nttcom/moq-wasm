use bytes::Bytes;
use gstreamer::prelude::*;
use gstreamer_app::prelude::GstBinExt;

pub(crate) struct GStreamerSender {
    gst_pipeline: gstreamer::Bin,
    _appsink: gstreamer_app::AppSink,
}

impl GStreamerSender {
    pub(crate) fn new(
        event_sender: tokio::sync::mpsc::Sender<(u64, Bytes)>,
    ) -> anyhow::Result<Self> {
        let pipeline_str = "videotestsrc ! videoconvert ! x264enc key-int-max=30 ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream ! appsink name=sink";
        let pipeline = gstreamer::parse::launch(pipeline_str).unwrap();
        let pipeline = pipeline.downcast::<gstreamer::Bin>().unwrap();
        let appsink = pipeline
            .by_name("sink")
            .unwrap()
            .dynamic_cast::<gstreamer_app::AppSink>()
            .unwrap();

        let mut id = 0;
        appsink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    tracing::info!("New sample received from appsink");
                    let sample = appsink
                        .pull_sample()
                        .map_err(|_| gstreamer::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gstreamer::FlowError::Error)?;

                    // キーフレーム（Iフレーム）かどうか判定
                    let is_keyframe = !buffer.flags().contains(gstreamer::BufferFlags::DELTA_UNIT);

                    if is_keyframe {
                        id += 1;
                        println!("🔑 key frame");
                    } else {
                        println!("📹 delta frame (P/B frame)");
                    }

                    let map = buffer
                        .map_readable()
                        .map_err(|_| gstreamer::FlowError::Error)?;
                    match event_sender.try_send((id, Bytes::copy_from_slice(map.as_slice()))) {
                        Ok(_) => tracing::info!("Sent buffer for group_id: {}", id),
                        Err(e) => {
                            tracing::error!("Failed to send buffer for group_id: {}: {}", id, e)
                        }
                    }
                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );

        Ok(Self {
            gst_pipeline: pipeline,
            _appsink: appsink,
        })
    }

    pub(crate) fn start(&self) -> anyhow::Result<()> {
        self.gst_pipeline.set_state(gstreamer::State::Playing)?;
        Ok(())
    }

    pub(crate) fn stop(&self) -> anyhow::Result<()> {
        self.gst_pipeline.set_state(gstreamer::State::Null)?;
        Ok(())
    }
}
