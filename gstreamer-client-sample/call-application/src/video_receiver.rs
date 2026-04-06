use gstreamer::prelude::*;
use gstreamer_app::prelude::GstBinExt;

pub(crate) struct VideoReceiver {
    gst_pipeline: gstreamer::Bin,
    join_handle: tokio::task::JoinHandle<()>,
}

impl VideoReceiver {
    pub(crate) fn new(data_receiver: moqt::DataReceiver<moqt::QUIC>) -> anyhow::Result<Self> {
        gstreamer::init().unwrap();
        let pipeline_str = "appsrc name=src is-live=true format=time do-timestamp=true ! h264parse ! avdec_h264 ! videoconvert ! osxvideosink sync=false";
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
        match data_receiver {
            moqt::DataReceiver::Stream(factory) => Self::handle_stream(factory, appsrc),
            moqt::DataReceiver::Datagram(datagram) => Self::handle_datagram(datagram, appsrc),
        }
    }

    fn handle_stream(
        mut factory: moqt::StreamDataReceiverFactory<moqt::QUIC>,
        appsrc: gstreamer_app::AppSrc,
    ) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        let join_handle = tokio::spawn(async move {
            let mut join_set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
            loop {
                match factory.next().await {
                    Ok(mut stream_receiver) => {
                        let appsrc_clone = appsrc.clone();
                        join_set.spawn(async move {
                            let mut group_id = 0u64;
                            while let Ok(object) = stream_receiver.receive().await {
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
                                            moqt::SubgroupObject::Payload { length: _, data } => {
                                                data
                                            }
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
                                let result = appsrc_clone.push_buffer(buffer);
                                tracing::info!(
                                    "Pushed buffer for group_id: {} with result: {:?}",
                                    group_id,
                                    result
                                );
                            }
                        });
                    }
                    Err(e) => {
                        tracing::info!("No more streams: {}", e);
                        break;
                    }
                }
            }
            join_set.join_all().await;
            let _ = appsrc.end_of_stream();
        });
        Ok(join_handle)
    }

    fn handle_datagram(
        mut data_receiver: moqt::DatagramReceiver<moqt::QUIC>,
        appsrc: gstreamer_app::AppSrc,
    ) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        let join_handle = tokio::spawn(async move {
            while let Ok(object) = data_receiver.receive().await {
                let field = object.field;
                let group_id = object.group_id;
                let bytes = match field.payload() {
                    moqt::ObjectDatagramPayload::Payload(data) => data,
                    moqt::ObjectDatagramPayload::Status(status) => {
                        tracing::info!(
                            "Received status: group_id={}, object_id={}, status={:?}",
                            group_id,
                            field.object_id().unwrap(),
                            status
                        );
                        continue;
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

impl Drop for VideoReceiver {
    fn drop(&mut self) {
        let _ = self.stop();
        self.join_handle.abort();
    }
}
