use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use bytes::Bytes;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};
use mediapack::loc::{LocExtension, TIMESTAMP_ID, to_extension_headers};
use moqt::{
    ClientConfig, Endpoint, QUIC, RequestErrorCode, Session, SessionEvent, TrackWriter,
    object::ContentExists, subscribe::SubscribeHandler,
};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::unix_micros;

pub(crate) const SOURCE_AUDIO_TRACK: &str = "audio";

pub(crate) struct SourceStreamer {
    _endpoint: Endpoint<QUIC>,
    join_handle: JoinHandle<()>,
}

impl SourceStreamer {
    pub(crate) async fn run(relay_url: &str, namespace: &str) -> Result<Self> {
        let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig {
            verify_certificate: false,
            ..ClientConfig::default()
        })?;
        let session = Arc::new(endpoint.connect(relay_url).await?.await?);
        session
            .publisher()
            .publish_namespace(namespace.to_string())
            .await
            .context("announce the source broadcast")?;
        tracing::info!(%namespace, "source broadcast announced");
        let catalog = catalog_payload(namespace)?;
        let join_handle = tokio::spawn(serve(session, catalog));
        Ok(Self {
            _endpoint: endpoint,
            join_handle,
        })
    }
}

impl Drop for SourceStreamer {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

fn catalog_payload(namespace: &str) -> Result<Bytes> {
    let catalog = serde_json::json!({
        "version": 1,
        "tracks": [
            { "namespace": namespace, "name": "video", "packaging": "loc", "role": "video", "isLive": true, "codec": "avc1.640028" },
            { "namespace": namespace, "name": SOURCE_AUDIO_TRACK, "packaging": "loc", "role": "audio", "isLive": true, "codec": "opus", "bitrate": 128000, "samplerate": 48000, "channelConfig": "mono" }
        ]
    });
    Ok(Bytes::from(serde_json::to_vec(&catalog)?))
}

async fn serve(session: Arc<Session>, catalog: Bytes) {
    let mut writers = Vec::new();
    let mut tone = None;
    loop {
        let event = match session.receive_event().await {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(?error, "source session ended");
                return;
            }
        };
        let SessionEvent::Subscribe(handler) = event else {
            continue;
        };
        let result = match handler.track_name.as_str() {
            "catalog" => accept(&session, &handler)
                .await
                .map(|writer| writers.push(spawn_catalog_writer(writer, catalog.clone()))),
            SOURCE_AUDIO_TRACK => accept(&session, &handler)
                .await
                .and_then(tone_task)
                .map(|task| tone = Some(task)),
            _ => {
                let _ = handler
                    .error(
                        RequestErrorCode::TrackDoesNotExist,
                        "unknown track".to_string(),
                    )
                    .await;
                Ok(())
            }
        };
        if let Err(error) = result {
            tracing::warn!(?error, "serve source SUBSCRIBE");
        }
    }
}

fn spawn_catalog_writer(mut writer: TrackWriter, catalog: Bytes) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(error) = writer.write_group(catalog).await {
            tracing::warn!(?error, "write source catalog");
        }
        std::future::pending::<()>().await;
    })
}

async fn accept(session: &Session, handler: &SubscribeHandler) -> Result<TrackWriter> {
    let track_alias = handler.ok(0, ContentExists::False).await?;
    let subscription = handler.into_subscription(track_alias);
    Ok(TrackWriter::new(
        session.publisher().create_stream(&subscription),
        0,
    ))
}

struct ToneTask {
    _pipeline: gst::Pipeline,
    _join_handle: JoinHandle<()>,
}

fn tone_task(mut writer: TrackWriter) -> Result<ToneTask> {
    gst::init()?;
    let pipeline = gst::parse::launch(
        "audiotestsrc is-live=true wave=sine freq=440 volume=0.3 \
         ! audio/x-raw,format=S16LE,rate=48000,channels=1 \
         ! opusenc frame-size=20 bitrate=128000 ! appsink name=sink sync=false",
    )?
    .downcast::<gst::Pipeline>()
    .map_err(|_| anyhow!("tone description did not produce a pipeline"))?;
    let sink = pipeline
        .by_name("sink")
        .context("missing appsink")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow!("sink is not an appsink"))?;
    let (packet_sender, mut packet_receiver) = mpsc::unbounded_channel::<Vec<u8>>();
    sink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                packet_sender
                    .send(map.as_slice().to_vec())
                    .map_err(|_| gst::FlowError::Eos)?;
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    pipeline.set_state(gst::State::Playing)?;
    let join_handle = tokio::spawn(async move {
        if writer.start_group_at(0).await.is_err() {
            return;
        }
        while let Some(packet) = packet_receiver.recv().await {
            let capture = LocExtension::varint(TIMESTAMP_ID, unix_micros());
            if let Err(error) = writer
                .write_with_extension_headers(Bytes::from(packet), to_extension_headers(&[capture]))
                .await
            {
                tracing::warn!(?error, "write source audio");
                return;
            }
        }
    });
    Ok(ToneTask {
        _pipeline: pipeline,
        _join_handle: join_handle,
    })
}
