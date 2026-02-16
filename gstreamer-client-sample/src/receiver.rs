use bytes::Bytes;
use gstreamer::{bus::BusWatchGuard, glib, prelude::*};
use gstreamer_app::prelude::GstBinExt;

pub(crate) struct GStreamerReceiver {
    gst_pipeline: gstreamer::Bin,
    join_handle: tokio::task::JoinHandle<()>,
}

impl GStreamerReceiver {
    pub(crate) fn new(
        mut receiver: tokio::sync::mpsc::Receiver<(u64, Bytes)>,
    ) -> anyhow::Result<Self> {
        // macOSではautovideosinkの代わりにosxvideosinkを明示的に指定する方が安定することがある
        let pipeline_str =
            "appsrc name=src ! h264parse ! avdec_h264 ! videoconvert ! osxvideosink";
        let pipeline = gstreamer::parse::launch(pipeline_str).unwrap();
        let pipeline = pipeline.downcast::<gstreamer::Bin>().unwrap();
        let appsrc = pipeline
            .by_name("src")
            .unwrap()
            .dynamic_cast::<gstreamer_app::AppSrc>()
            .unwrap();

        // let caps = gstreamer::Caps::builder("video/x-h264")
        //     .field("stream-format", "byte-stream")
        //     .build();
        // appsrc.set_caps(Some(&caps));
        // appsrc.set_is_live(true);
        // appsrc.set_do_timestamp(true);

        let join_handle = tokio::spawn(async move {
            while let Some((group_id, bytes)) = receiver.recv().await {
                tracing::info!("Received buffer for group_id: {}", group_id);
                let mut buffer = gstreamer::Buffer::with_size(bytes.len()).unwrap();
                {
                    let buffer_ref = buffer.get_mut().unwrap();
                    let mut map = buffer_ref.map_writable().unwrap();
                    map.copy_from_slice(&bytes);
                }
                let result = appsrc.push_buffer(buffer);
                tracing::info!(
                    "Pushed buffer for group_id: {} with result: {:?}",
                    group_id,
                    result
                );
            }
            let _ = appsrc.end_of_stream();
        });

        Ok(Self {
            gst_pipeline: pipeline,
            join_handle,
        })
    }

    pub(crate) fn start(&self) -> anyhow::Result<()> {
        self.gst_pipeline.set_state(gstreamer::State::Playing)?;
        Ok(())
    }

    pub(crate) fn stop(&self) -> anyhow::Result<()> {
        self.gst_pipeline.set_state(gstreamer::State::Null)?;
        self.join_handle.abort();
        Ok(())
    }
}
