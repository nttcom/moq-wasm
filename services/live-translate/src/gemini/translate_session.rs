use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use tokio::{net::TcpStream, sync::mpsc, task::JoinHandle};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{Message, client::IntoClientRequest, http::header::AUTHORIZATION},
};

use super::{
    AgentPlatform,
    messages::{ServerEvent, audio_message, parse_server_message, setup_message},
};
use crate::{pcm_framer::PcmFramer, translation_engine::TranslationOutput};

const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const INPUT_CHUNK_SAMPLES: usize = 1_600;

type GeminiSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

enum ConnectionEnd {
    InputClosed,
    Reconnect,
}

pub(crate) struct GeminiTranslation {
    join_handle: JoinHandle<()>,
}

impl GeminiTranslation {
    pub(crate) fn run(
        platform: Arc<AgentPlatform>,
        target_language_code: &'static str,
        pcm_receiver: mpsc::Receiver<Vec<i16>>,
        translated_sender: mpsc::Sender<TranslationOutput>,
    ) -> Self {
        let join_handle = tokio::spawn(translate(
            platform,
            target_language_code,
            pcm_receiver,
            translated_sender,
        ));
        Self { join_handle }
    }
}

impl Drop for GeminiTranslation {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn translate(
    platform: Arc<AgentPlatform>,
    target_language_code: &'static str,
    mut pcm_receiver: mpsc::Receiver<Vec<i16>>,
    translated_sender: mpsc::Sender<TranslationOutput>,
) {
    let mut resumption_handle = None;
    loop {
        match translate_over_one_connection(
            &platform,
            target_language_code,
            &mut resumption_handle,
            &mut pcm_receiver,
            &translated_sender,
        )
        .await
        {
            Ok(ConnectionEnd::InputClosed) => return,
            Ok(ConnectionEnd::Reconnect) => {
                tracing::info!(
                    target_language_code,
                    "Gemini connection ending, reconnecting"
                );
            }
            Err(error) => {
                tracing::warn!(
                    target_language_code,
                    ?error,
                    "Gemini connection failed, retrying"
                );
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        }
    }
}

async fn translate_over_one_connection(
    platform: &AgentPlatform,
    target_language_code: &str,
    resumption_handle: &mut Option<String>,
    pcm_receiver: &mut mpsc::Receiver<Vec<i16>>,
    translated_sender: &mpsc::Sender<TranslationOutput>,
) -> Result<ConnectionEnd> {
    let mut socket = connect(platform).await?;
    socket
        .send(Message::text(setup_message(
            &platform.model,
            target_language_code,
            resumption_handle.as_deref(),
        )))
        .await
        .context("send Gemini setup")?;
    let mut setup_complete = false;
    let mut input_chunks = PcmFramer::new(INPUT_CHUNK_SAMPLES);
    loop {
        tokio::select! {
            samples = pcm_receiver.recv() => {
                let Some(samples) = samples else {
                    let _ = socket.close(None).await;
                    return Ok(ConnectionEnd::InputClosed);
                };
                if !setup_complete {
                    continue;
                }
                for chunk in input_chunks.push(&samples) {
                    socket
                        .send(Message::text(audio_message(&chunk)))
                        .await
                        .context("send audio to Gemini")?;
                }
            }
            message = socket.next() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text.to_string(),
                    Some(Ok(Message::Binary(bytes))) => String::from_utf8(bytes.to_vec()).context("Gemini sent non-UTF-8 binary")?,
                    Some(Ok(Message::Close(frame))) => bail!("Gemini closed the connection: {frame:?}"),
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => return Err(error).context("receive from Gemini"),
                    None => bail!("Gemini connection ended"),
                };
                for event in parse_server_message(&text)? {
                    match event {
                        ServerEvent::SetupComplete => {
                            tracing::info!(target_language_code, "Gemini session ready");
                            setup_complete = true;
                            if translated_sender.send(TranslationOutput::ConnectionStarted).await.is_err() {
                                let _ = socket.close(None).await;
                                return Ok(ConnectionEnd::InputClosed);
                            }
                        }
                        ServerEvent::TranslatedAudio(samples) => {
                            if translated_sender.send(TranslationOutput::Audio(samples)).await.is_err() {
                                let _ = socket.close(None).await;
                                return Ok(ConnectionEnd::InputClosed);
                            }
                        }
                        ServerEvent::InputTranscript(transcript) => {
                            tracing::debug!(target_language_code, text = %transcript.text, "Gemini heard");
                            if translated_sender.send(TranslationOutput::SourceTranscript(transcript)).await.is_err() {
                                let _ = socket.close(None).await;
                                return Ok(ConnectionEnd::InputClosed);
                            }
                        }
                        ServerEvent::OutputTranscript(transcript) => {
                            tracing::debug!(target_language_code, text = %transcript.text, "Gemini said");
                            if translated_sender.send(TranslationOutput::TranslatedTranscript(transcript)).await.is_err() {
                                let _ = socket.close(None).await;
                                return Ok(ConnectionEnd::InputClosed);
                            }
                        }
                        ServerEvent::ResumptionHandle(handle) => *resumption_handle = Some(handle),
                        ServerEvent::GoAway => return Ok(ConnectionEnd::Reconnect),
                    }
                }
            }
        }
    }
}

/// A token is fetched per connection: a session lasts about ten minutes and
/// the service account's token an hour.
async fn connect(platform: &AgentPlatform) -> Result<GeminiSocket> {
    let token = platform
        .bearer_token()
        .await
        .context("get the Agent Platform access token")?;
    let mut request = platform
        .endpoint
        .as_str()
        .into_client_request()
        .context("build the Gemini Live API request")?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {token}")
            .parse()
            .context("the access token is not a valid header value")?,
    );
    let (socket, _) = connect_async_tls_with_config(request, None, false, Some(tls_connector()?))
        .await
        .context("connect to the Gemini Live API")?;
    Ok(socket)
}

fn tls_connector() -> Result<Connector> {
    let mut roots = rustls::RootCertStore::empty();
    for certificate in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(certificate);
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .context("configure TLS for Gemini")?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}
