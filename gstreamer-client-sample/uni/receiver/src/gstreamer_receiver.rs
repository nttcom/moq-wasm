use gstreamer::prelude::*;
use gstreamer_app::prelude::GstBinExt;

pub(crate) struct GStreamerReceiver {
    gst_pipeline: gstreamer::Bin,
    join_handle: tokio::task::JoinHandle<()>,
}

impl GStreamerReceiver {
    pub(crate) fn new(data_receiver: moqt::DataReceiver<moqt::QUIC>) -> anyhow::Result<Self> {
        gstreamer::init().unwrap();
        let pipeline_str = "appsrc name=src ! h264parse ! avdec_h264 ! videoconvert ! osxvideosink";
        let pipeline = gstreamer::parse::launch(pipeline_str).unwrap();
        let pipeline = pipeline.downcast::<gstreamer::Bin>().unwrap();
        let appsrc = pipeline
            .by_name("src")
            .unwrap()
            .dynamic_cast::<gstreamer_app::AppSrc>()
            .unwrap();

        let join_handle = Self::set_appsrc_callbacks_for_stream(appsrc, data_receiver).unwrap();

        Ok(Self {
            gst_pipeline: pipeline,
            join_handle,
        })
    }

    fn set_appsrc_callbacks_for_stream(
        appsrc: gstreamer_app::AppSrc,
        data_receiver: moqt::DataReceiver<moqt::QUIC>,
    ) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        let mut data_receiver = match data_receiver {
            moqt::DataReceiver::Stream(stream) => stream,
            // moqt::DataReceiver::Datagram(datagram) => datagram,
            _ => anyhow::bail!("Unsupported data receiver type"),
        };

        let join_handle = tokio::spawn(async move {
            let mut group_id = 0;
            while let Ok(object) = data_receiver.receive().await {
                let bytes = match object {
                    moqt::Subgroup::Header(subgroup_header) => {
                        tracing::info!(
                            "Received subgroup header: group_id={}",
                            subgroup_header.group_id
                        );
                        group_id = subgroup_header.group_id;
                        continue;
                    }
                    moqt::Subgroup::Object(subgroup_object_field) => {
                        tracing::info!(
                            "Received subgroup object: group_id={}, object_id={}",
                            group_id,
                            subgroup_object_field.object_id_delta
                        );

                        match subgroup_object_field.subgroup_object {
                            moqt::SubgroupObject::Payload(payload) => payload,
                            _ => {
                                tracing::error!("Unsupported subgroup object type");
                                continue;
                            }
                        }
                    }
                };

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
        Ok(join_handle)
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

impl Drop for GStreamerReceiver {
    fn drop(&mut self) {
        let _ = self.stop();
        self.join_handle.abort();
    }
}
