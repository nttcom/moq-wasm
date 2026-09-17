use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result, anyhow};
use gstreamer as gst;
use gstreamer::{glib, prelude::*, subclass::prelude::*};
use media_publisher::{MediaPublisher, MoqtManager, MoqtTarget};
use mediapack::Timestamp;
use tokio::runtime::Runtime;

use crate::media_input::{VideoInput, audio_config, audio_sample};

const VIDEO_PAD: &str = "video";
const AUDIO_PAD: &str = "audio";

#[derive(Default)]
struct Settings {
    relay_url: Option<String>,
    namespace: Option<String>,
    auth_token: Option<String>,
}

struct Started {
    runtime: Runtime,
    publisher: MediaPublisher,
    video: VideoInput,
}

#[derive(Default)]
struct Pads {
    video: Option<gst::Pad>,
    audio: Option<gst::Pad>,
    ended: Vec<String>,
}

impl Pads {
    fn slot(&mut self, name: &str) -> Option<&mut Option<gst::Pad>> {
        match name {
            VIDEO_PAD => Some(&mut self.video),
            AUDIO_PAD => Some(&mut self.audio),
            _ => None,
        }
    }

    fn all_ended(&self) -> bool {
        [&self.video, &self.audio]
            .into_iter()
            .flatten()
            .all(|pad| self.ended.iter().any(|name| *name == pad.name()))
    }
}

#[derive(Default)]
pub struct MoqtSink {
    settings: Mutex<Settings>,
    started: Mutex<Option<Started>>,
    pads: Mutex<Pads>,
}

#[glib::object_subclass]
impl ObjectSubclass for MoqtSink {
    const NAME: &'static str = "GstMoqtSink";
    type Type = super::MoqtSink;
    type ParentType = gst::Element;
}

impl ObjectImpl for MoqtSink {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
            vec![
                glib::ParamSpecString::builder("relay-url")
                    .nick("Relay URL")
                    .blurb("MoQT relay URL: moqt://host:port for QUIC or https://host:port for WebTransport")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecString::builder("namespace")
                    .nick("Track namespace")
                    .blurb("Slash-separated MoQT track namespace to publish under, e.g. anon/live/test")
                    .mutable_ready()
                    .build(),
                glib::ParamSpecString::builder("auth-token")
                    .nick("Authorization token")
                    .blurb("JWT presented to the relay in CLIENT_SETUP")
                    .mutable_ready()
                    .build(),
            ]
        });
        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        let mut settings = self.settings.lock().expect("settings lock");
        let text = value.get().expect("type checked upstream");
        match pspec.name() {
            "relay-url" => settings.relay_url = text,
            "namespace" => settings.namespace = text,
            "auth-token" => settings.auth_token = text,
            _ => unreachable!("property {} is not declared", pspec.name()),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        let settings = self.settings.lock().expect("settings lock");
        match pspec.name() {
            "relay-url" => settings.relay_url.to_value(),
            "namespace" => settings.namespace.to_value(),
            "auth-token" => settings.auth_token.to_value(),
            _ => unreachable!("property {} is not declared", pspec.name()),
        }
    }

    fn constructed(&self) {
        self.parent_constructed();
        self.obj().set_element_flags(gst::ElementFlags::SINK);
    }
}

impl GstObjectImpl for MoqtSink {}

impl ElementImpl for MoqtSink {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
            gst::subclass::ElementMetadata::new(
                "MoQT sink",
                "Sink/Network",
                "Publishes H.264 video and AAC audio into a MoQT relay as LOC and CMAF tracks",
                "NTT Communications",
            )
        });
        Some(&METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
            let video = gst::Caps::builder("video/x-h264")
                .field("stream-format", "byte-stream")
                .field("alignment", "au")
                .build();
            let audio = gst::Caps::builder("audio/mpeg")
                .field("mpegversion", 4i32)
                .field("stream-format", "raw")
                .build();
            vec![
                gst::PadTemplate::new(
                    VIDEO_PAD,
                    gst::PadDirection::Sink,
                    gst::PadPresence::Request,
                    &video,
                )
                .expect("static video pad template"),
                gst::PadTemplate::new(
                    AUDIO_PAD,
                    gst::PadDirection::Sink,
                    gst::PadPresence::Request,
                    &audio,
                )
                .expect("static audio pad template"),
            ]
        });
        TEMPLATES.as_ref()
    }

    fn request_new_pad(
        &self,
        templ: &gst::PadTemplate,
        _name: Option<&str>,
        _caps: Option<&gst::Caps>,
    ) -> Option<gst::Pad> {
        let mut pads = self.pads.lock().expect("pads lock");
        let slot = pads.slot(templ.name_template())?;
        if slot.is_some() {
            return None;
        }
        let pad = gst::Pad::builder_from_template(templ)
            .chain_function(|pad, parent, buffer| {
                MoqtSink::catch_panic_pad_function(
                    parent,
                    || Err(gst::FlowError::Error),
                    |sink| sink.sink_chain(pad, buffer),
                )
            })
            .event_function(|pad, parent, event| {
                MoqtSink::catch_panic_pad_function(
                    parent,
                    || false,
                    |sink| sink.sink_event(pad, event),
                )
            })
            .build();
        if self.obj().current_state() > gst::State::Ready {
            let _ = pad.set_active(true);
        }
        self.obj().add_pad(&pad).ok()?;
        *slot = Some(pad.clone());
        Some(pad)
    }

    fn release_pad(&self, pad: &gst::Pad) {
        {
            let mut pads = self.pads.lock().expect("pads lock");
            if let Some(slot) = pads.slot(&pad.name()) {
                *slot = None;
            }
        }
        let _ = pad.set_active(false);
        let _ = self.obj().remove_pad(pad);
    }

    fn change_state(
        &self,
        transition: gst::StateChange,
    ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
        if transition == gst::StateChange::ReadyToPaused
            && let Err(err) = self.start()
        {
            gst::element_imp_error!(self, gst::ResourceError::OpenWrite, ["{err:#}"]);
            return Err(gst::StateChangeError);
        }
        let success = self.parent_change_state(transition)?;
        match transition {
            gst::StateChange::ReadyToPaused | gst::StateChange::PlayingToPaused => {
                Ok(gst::StateChangeSuccess::NoPreroll)
            }
            gst::StateChange::PausedToReady => {
                self.stop();
                Ok(success)
            }
            _ => Ok(success),
        }
    }
}

impl MoqtSink {
    fn start(&self) -> Result<()> {
        let (target, namespace) = {
            let settings = self.settings.lock().expect("settings lock");
            let url = settings
                .relay_url
                .clone()
                .context("the relay-url property is required")?;
            let namespace = settings
                .namespace
                .clone()
                .context("the namespace property is required")?;
            (
                MoqtTarget {
                    url,
                    auth_token: settings.auth_token.clone(),
                },
                namespace,
            )
        };
        let namespace: Vec<String> = namespace
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();
        if namespace.is_empty() {
            return Err(anyhow!("the namespace property must not be empty"));
        }
        let runtime = Runtime::new().context("start the tokio runtime")?;
        let manager = MoqtManager::new(Some(target));
        runtime
            .block_on(manager.setup_namespace(&namespace))
            .context("connect to the relay and publish the namespace")?;
        tracing::info!(namespace = %namespace.join("/"), "moqtsink connected");
        *self.started.lock().expect("started lock") = Some(Started {
            runtime,
            publisher: MediaPublisher::new(manager, namespace),
            video: VideoInput::default(),
        });
        Ok(())
    }

    fn stop(&self) {
        self.started.lock().expect("started lock").take();
        self.pads.lock().expect("pads lock").ended.clear();
    }

    fn sink_chain(
        &self,
        pad: &gst::Pad,
        buffer: gst::Buffer,
    ) -> Result<gst::FlowSuccess, gst::FlowError> {
        let Some(pts) = buffer.pts() else {
            tracing::warn!(pad = %pad.name(), "buffer without pts dropped");
            return Ok(gst::FlowSuccess::Ok);
        };
        let pts = Timestamp::from_micros(pts.useconds());
        let dts = buffer
            .dts()
            .map_or(pts, |dts| Timestamp::from_micros(dts.useconds()));
        let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
        let mut started = self.started.lock().expect("started lock");
        let Some(started) = started.as_mut() else {
            return Err(gst::FlowError::Flushing);
        };
        let events = match pad.name().as_str() {
            VIDEO_PAD => started
                .video
                .push(map.as_slice(), pts, dts)
                .map_err(|err| self.stream_error(err))?,
            _ => vec![audio_sample(map.as_slice(), pts)],
        };
        for event in &events {
            started
                .runtime
                .block_on(started.publisher.push(event))
                .map_err(|err| self.stream_error(err))?;
        }
        Ok(gst::FlowSuccess::Ok)
    }

    fn sink_event(&self, pad: &gst::Pad, event: gst::Event) -> bool {
        match event.view() {
            gst::EventView::Caps(caps) if pad.name() == AUDIO_PAD => {
                return match self.publish_audio_config(caps.caps()) {
                    Ok(()) => true,
                    Err(err) => {
                        self.report_stream_error(err);
                        false
                    }
                };
            }
            gst::EventView::Eos(_) => {
                let all_ended = {
                    let mut pads = self.pads.lock().expect("pads lock");
                    pads.ended.push(pad.name().to_string());
                    pads.all_ended()
                };
                if all_ended {
                    tracing::info!("all pads reached end of stream");
                    let _ = self
                        .obj()
                        .post_message(gst::message::Eos::builder().src(&*self.obj()).build());
                }
                return true;
            }
            _ => {}
        }
        gst::Pad::event_default(pad, Some(&*self.obj()), event)
    }

    fn publish_audio_config(&self, caps: &gst::CapsRef) -> Result<()> {
        let codec_data = caps
            .structure(0)
            .and_then(|structure| structure.get::<gst::Buffer>("codec_data").ok())
            .context("audio caps carry no codec_data (AudioSpecificConfig)")?;
        let codec_data = codec_data
            .map_readable()
            .map_err(|_| anyhow!("map codec_data"))?;
        let event = audio_config(codec_data.as_slice())?;
        let mut started = self.started.lock().expect("started lock");
        let started = started
            .as_mut()
            .context("audio caps received before the sink started")?;
        started.runtime.block_on(started.publisher.push(&event))
    }

    fn stream_error(&self, err: anyhow::Error) -> gst::FlowError {
        self.report_stream_error(err);
        gst::FlowError::Error
    }

    fn report_stream_error(&self, err: anyhow::Error) {
        gst::element_imp_error!(self, gst::StreamError::Failed, ["{err:#}"]);
    }
}
