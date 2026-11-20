//! Places calls through the SIP gateway to the local Asterisk trunk
//! (`asterisk/extensions.conf`) and checks what a page would see: the call's
//! states on its `call` track, the phone's audio, and the call ending when
//! either side hangs up. Run through `run.sh`.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail, ensure};
use broadcast::{Broadcast, BroadcastReader, BroadcastWriter, RelayConnection, SourceTracks};
use bytes::Bytes;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks, AppSrc};
use mediapack::{AudioConfig, AudioSample, MediaEvent, Timestamp};
use moqt::{
    CertificateVerification, ClientConfig,
    client::{self, GroupStartOptions, NamespaceWatch, Subscription},
};
use serde_json::{Value, json};
use tokio::{sync::mpsc, time::timeout};

const ROOM: &str = "sip-e2e/room";
const ECHO: &str = "+819000000001";
const BUSY: &str = "+819000000003";
const HANGS_UP: &str = "+819000000004";
const NOT_ALLOWED: &str = "+12025550100";
/// Inside the telephone band, far from Asterisk's own 1 kHz tone.
const TONE_HZ: f64 = 700.0;
const STATE_TIMEOUT: Duration = Duration::from_secs(15);

struct Env {
    relay_url: String,
    gateway_url: String,
    /// Publishes and subscribes in the room.
    page_token: String,
    /// The app server's token, with the `calls` claim for the room.
    call_token: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    gst::init()?;
    let var = |name: &str| std::env::var(name).with_context(|| format!("{name} must be set"));
    let env = Env {
        relay_url: var("E2E_RELAY_URL")?,
        gateway_url: var("E2E_GATEWAY_URL")?,
        page_token: var("E2E_PAGE_TOKEN")?,
        call_token: var("E2E_CALL_TOKEN")?,
    };

    a_destination_outside_the_app_s_prefixes_is_refused(&env).await?;
    println!("ok: a destination outside the app's prefixes is refused");
    the_phone_hears_the_page_and_the_page_hangs_up(&env).await?;
    println!("ok: the phone hears the page and the page hangs up");
    a_busy_phone_ends_the_call_with_486(&env).await?;
    println!("ok: a busy phone ends the call with 486");
    the_phone_hangs_up(&env).await?;
    println!("ok: the phone hangs up");
    println!("sip-gateway e2e passed");
    Ok(())
}

async fn a_destination_outside_the_app_s_prefixes_is_refused(env: &Env) -> Result<()> {
    let response = place(env, NOT_ALLOWED, &format!("{ROOM}/alice/refused")).await?;
    ensure!(
        response.status() == reqwest::StatusCode::FORBIDDEN,
        "expected 403, got {}",
        response.status()
    );
    Ok(())
}

async fn the_phone_hears_the_page_and_the_page_hangs_up(env: &Env) -> Result<()> {
    let peer = format!("{ROOM}/alice/echo");
    let page = connect(env).await?;
    let peer_session = connect(env).await?;
    let tone = ToneBroadcast::run(&peer_session, &peer).await?;
    let call = CallObserver::place(env, &page, ECHO, &peer).await?;
    call.until_state("answered").await?;

    let heard = phone_frequency(&page, &call.namespace).await?;
    ensure!(
        (heard - TONE_HZ).abs() < 60.0,
        "the phone sent back {heard:.0} Hz instead of the page's {TONE_HZ} Hz"
    );

    drop(tone);
    peer_session.close().await;
    let reason = call.until_state("ended").await?;
    ensure!(reason == "hangup", "ended with {reason}");
    call.until_gone().await
}

async fn a_busy_phone_ends_the_call_with_486(env: &Env) -> Result<()> {
    let page = connect(env).await?;
    let call = CallObserver::place(env, &page, BUSY, &format!("{ROOM}/alice/busy")).await?;
    let reason = call.until_state("ended").await?;
    ensure!(reason == "486", "ended with {reason}");
    call.until_gone().await
}

async fn the_phone_hangs_up(env: &Env) -> Result<()> {
    let page = connect(env).await?;
    let call = CallObserver::place(env, &page, HANGS_UP, &format!("{ROOM}/alice/hangs-up")).await?;
    call.until_state("answered").await?;
    let reason = call.until_state("ended").await?;
    ensure!(reason == "hangup", "ended with {reason}");
    call.until_gone().await
}

async fn connect(env: &Env) -> Result<Arc<client::Session>> {
    Ok(Arc::new(
        client::Session::connect(
            &env.relay_url,
            &ClientConfig {
                certificate_verification: CertificateVerification::Disabled,
                authorization_token: Some(env.page_token.clone().into()),
                ..ClientConfig::default()
            },
        )
        .await?,
    ))
}

async fn place(env: &Env, to: &str, peer: &str) -> Result<reqwest::Response> {
    Ok(reqwest::Client::new()
        .post(format!("{}/calls", env.gateway_url))
        .bearer_auth(&env.call_token)
        .json(&json!({ "to": to, "namespace_prefix": ROOM, "peer": peer }))
        .send()
        .await?)
}

/// Reads a call's states as a page does.
struct CallObserver {
    namespace: String,
    phones: NamespaceWatch,
    states: Subscription,
}

impl CallObserver {
    async fn place(env: &Env, page: &client::Session, to: &str, peer: &str) -> Result<Self> {
        let phones = page.watch_namespaces(&["sip-e2e", "room", "phone"]).await?;
        let response = place(env, to, peer).await?;
        ensure!(
            response.status() == reqwest::StatusCode::CREATED,
            "expected 201, got {}",
            response.status()
        );
        let body: Value = response.json().await?;
        let namespace = body["namespace"]
            .as_str()
            .context("no namespace in the answer")?
            .to_string();
        let elements: Vec<&str> = namespace.split('/').collect();
        until_namespace(&phones, &elements, true)
            .await
            .context("the call's broadcast did not appear")?;
        let states = page
            .subscribe_current_group(&elements, "call", GroupStartOptions::default())
            .await?;
        Ok(Self {
            namespace,
            phones,
            states,
        })
    }

    /// Resolves once the gateway ended the call's broadcast, after which the
    /// call no longer counts against the app's limit.
    async fn until_gone(&self) -> Result<()> {
        let elements: Vec<&str> = self.namespace.split('/').collect();
        until_namespace(&self.phones, &elements, false)
            .await
            .context("the call's broadcast did not end")
    }

    /// The `reason` of the first state `expected` (empty when it has none);
    /// fails when the call ends first or after `STATE_TIMEOUT`.
    async fn until_state(&self, expected: &str) -> Result<String> {
        timeout(STATE_TIMEOUT, async {
            loop {
                let object = self.states.next().await?.context("the call track ended")?;
                let state: Value = serde_json::from_slice(&object.payload)?;
                let name = state["state"].as_str().unwrap_or_default();
                let reason = state["reason"].as_str().unwrap_or_default().to_string();
                if name == expected {
                    return Ok(reason);
                }
                if name == "ended" {
                    bail!("ended with {reason} before {expected}");
                }
            }
        })
        .await
        .with_context(|| format!("no {expected} state"))?
    }
}

/// Resolves once `namespace` is announced (`active`) or withdrawn.
async fn until_namespace(phones: &NamespaceWatch, namespace: &[&str], active: bool) -> Result<()> {
    timeout(STATE_TIMEOUT, async {
        loop {
            match phones.next().await {
                Some(event) if event.namespace == namespace && event.active == active => {
                    return Ok(());
                }
                Some(_) => {}
                None => bail!("the relay session ended"),
            }
        }
    })
    .await?
}

/// The page's microphone: a sine tone, published as an Opus broadcast.
struct ToneBroadcast {
    pipeline: gst::Pipeline,
    _writer_task: tokio::task::JoinHandle<()>,
}

impl ToneBroadcast {
    async fn run(session: &client::Session, namespace: &str) -> Result<Self> {
        let connection = RelayConnection::with_client_session(session);
        session.add_handler(Arc::new(connection.clone()));
        let broadcast = Broadcast::new(
            connection,
            namespace.split('/').map(str::to_string).collect(),
        );
        broadcast.announce().await?;
        let mut writer = BroadcastWriter::run(
            broadcast,
            SourceTracks {
                cmaf: false,
                ..SourceTracks::default()
            },
        );
        let (pipeline, mut receiver) = pipeline_output(&format!(
            "audiotestsrc is-live=true wave=sine freq={TONE_HZ} volume=0.5 \
                 ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=1 \
                 ! opusenc frame-size=20 ! appsink name=sink sync=false"
        ))?;
        let writer_task = tokio::spawn(async move {
            let _ = writer.push(MediaEvent::AudioConfig(AudioConfig::Opus {
                sample_rate: 48_000,
                channels: 1,
            }));
            let mut frames = 0u64;
            while let Some(packet) = receiver.recv().await {
                let _ = writer.push(MediaEvent::Audio(AudioSample {
                    data: Bytes::from(packet),
                    pts: Timestamp::from_micros(frames * 20_000),
                    audio_level: None,
                }));
                frames += 1;
            }
        });
        Ok(Self {
            pipeline,
            _writer_task: writer_task,
        })
    }
}

impl Drop for ToneBroadcast {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

/// The dominant frequency of two seconds of the phone's audio, from its
/// zero crossings.
async fn phone_frequency(page: &client::Session, namespace: &str) -> Result<f64> {
    let mut reader = BroadcastReader::run(page.moqt_session(), namespace).await?;
    let (pipeline, mut pcm) = pipeline_output(
        "appsrc name=source format=time is-live=true do-timestamp=true \
           caps=audio/x-opus,channel-mapping-family=(int)0,rate=(int)48000,channels=(int)1 \
         ! opusdec ! audioconvert ! audio/x-raw,format=S16LE,rate=48000,channels=1 \
         ! appsink name=sink sync=false",
    )?;
    let source = pipeline
        .by_name("source")
        .context("missing appsrc")?
        .downcast::<AppSrc>()
        .map_err(|_| anyhow!("source is not an appsrc"))?;
    let mut samples: Vec<i16> = Vec::new();
    // The first second carries the echo's start and the jitter buffers filling.
    let wanted = 3 * 48_000;
    let result = timeout(Duration::from_secs(10), async {
        while samples.len() < wanted {
            tokio::select! {
                event = reader.next_event() => match event {
                    Some(MediaEvent::Audio(sample)) => {
                        source
                            .push_buffer(gst::Buffer::from_slice(sample.data.to_vec()))
                            .map_err(|error| anyhow!("push Opus: {error:?}"))?;
                    }
                    Some(_) => {}
                    None => bail!("the phone's broadcast ended"),
                },
                Some(chunk) = pcm.recv() => samples.extend(
                    chunk.as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)),
                ),
            }
        }
        Ok(())
    })
    .await;
    let _ = pipeline.set_state(gst::State::Null);
    result.context("not enough phone audio")??;
    let last_two_seconds = &samples[samples.len() - 2 * 48_000..];
    let crossings = last_two_seconds
        .windows(2)
        .filter(|pair| (pair[0] < 0) != (pair[1] < 0))
        .count();
    Ok(crossings as f64 / 2.0 / 2.0)
}

/// Starts `description` and hands out every buffer its `sink` pulls.
fn pipeline_output(description: &str) -> Result<(gst::Pipeline, mpsc::UnboundedReceiver<Vec<u8>>)> {
    let pipeline = gst::parse::launch(description)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("not a pipeline"))?;
    let sink = pipeline
        .by_name("sink")
        .context("missing appsink")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow!("sink is not an appsink"))?;
    let (sender, receiver) = mpsc::unbounded_channel();
    sink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                sender
                    .send(map.as_slice().to_vec())
                    .map_err(|_| gst::FlowError::Eos)?;
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    pipeline.set_state(gst::State::Playing)?;
    Ok((pipeline, receiver))
}
