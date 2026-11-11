use std::sync::Arc;

use anyhow::Context;
use moqt::{AcceptError, Endpoint, TraceContext, TransportProtocol, server::Accepting};
use tokio::sync::oneshot;
use tracing::{Instrument, Span};

use crate::{
    logging::{link_span, mark_current_span_failed, relay_hostname},
    modules::{
        auth::session_authenticator::SessionAuthenticator,
        domain::{
            session_id::{SessionId, generate_session_id},
            session_peer::SessionPeer,
        },
        session::session_repository::{NewSession, SessionRepository},
    },
};

#[derive(Clone)]
pub(crate) struct SessionIntake {
    pub(crate) repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    pub(crate) accepted_peer: SessionPeer,
    pub(crate) authenticator: Arc<SessionAuthenticator>,
}

impl SessionIntake {
    async fn establish(&self, session_id: SessionId, connecting: Accepting, session_span: Span) {
        let handshake = match connecting.await {
            Ok(handshake) => handshake,
            Err(error) => {
                tracing::warn!(
                    %error,
                    session_id,
                    session_peer = self.accepted_peer.kind(),
                    relay_hostname = relay_hostname(),
                    "failed to establish session"
                );
                return;
            }
        };
        let setup_span = session_setup_span(
            session_id,
            self.accepted_peer,
            &session_span,
            handshake.trace_context(),
        );
        let verified_token = match self
            .authenticator
            .authenticate(handshake.authorization_tokens(), &self.accepted_peer)
            .instrument(setup_span.clone())
            .await
        {
            Ok(verified_token) => {
                for span in [&session_span, &setup_span] {
                    span.record("app_id", verified_token.app_id.as_str());
                }
                if let Some(relay_id) = &verified_token.relay_id {
                    session_span.record("peer_relay_id", relay_id.as_str());
                }
                verified_token
            }
            Err(rejected) => {
                let description = format!("{:?}", rejected.code);
                async {
                    tracing::warn!(
                        code = ?rejected.code,
                        reason = %rejected.reason,
                        "session rejected at CLIENT_SETUP"
                    );
                    handshake.reject(rejected.code, &rejected.reason).await;
                    mark_current_span_failed(description.clone());
                }
                .instrument(setup_span)
                .await;
                mark_current_span_failed(description);
                return;
            }
        };
        let accepted = handshake.accept(&session_span).instrument(setup_span).await;
        let session = match accepted {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(%error, "failed to establish session");
                return;
            }
        };
        let session_add_span = tracing::debug_span!(
            parent: &session_span,
            "relay.session_repository.add",
            session_id = session_id
        );
        async {
            tracing::debug!("session accepted");
            self.repo.lock().await.add(NewSession {
                session_id,
                session: Box::new(session),
                session_span: session_span.clone(),
                peer: self.accepted_peer,
                verified_token,
            });
        }
        .instrument(session_add_span)
        .await;
    }
}

/// The CLIENT_SETUP decision and the SERVER_SETUP that answers it.
fn session_setup_span(
    session_id: SessionId,
    peer: SessionPeer,
    session_span: &Span,
    trace_context: Option<&TraceContext>,
) -> Span {
    let span = tracing::info_span!(
        parent: None,
        "relay.session.setup",
        session_id = session_id,
        session_peer = peer.kind(),
        relay_hostname = relay_hostname(),
        app_id = tracing::field::Empty,
    );
    if let Some(trace_context) = trace_context {
        telemetry::continue_trace_from_traceparent(&span, trace_context.as_str());
    }
    link_span(&span, session_span);
    span
}

pub struct SessionHandler {
    join_handle: tokio::task::JoinHandle<()>,
    stop_sender: Option<oneshot::Sender<()>>,
    #[cfg(test)]
    pub(crate) local_port: u16,
}

impl SessionHandler {
    pub(crate) fn run<T: TransportProtocol>(
        config: moqt::server::ServerConfig,
        intake: SessionIntake,
    ) -> anyhow::Result<Self> {
        let endpoint = Endpoint::<T>::create_server(&config).with_context(|| {
            format!(
                "failed to open the transport endpoint on port {}",
                config.port
            )
        })?;
        #[cfg(test)]
        let local_port = endpoint.local_port()?;
        let (stop_sender, mut stop_receiver) = oneshot::channel();
        let join_handle = tokio::task::Builder::new()
            .spawn(async move {
                let relay_hostname = relay_hostname();
                let session_peer = intake.accepted_peer.kind();
                loop {
                    let connecting = tokio::select! {
                        biased;
                        _ = &mut stop_receiver => {
                            tracing::info!(session_peer, relay_hostname, "stopped accepting sessions; refusing new connections");
                            endpoint.refuse_new_connections();
                            break;
                        }
                        connecting = endpoint.accept() => connecting,
                    };
                    let connecting = match connecting {
                        Ok(connecting) => connecting,
                        Err(error @ AcceptError::EndpointClosing) => {
                            tracing::info!(%error, session_peer, relay_hostname, "transport endpoint closed");
                            break;
                        }
                        Err(error) => {
                            tracing::warn!(%error, session_peer, relay_hostname, "failed to accept transport connection");
                            continue;
                        }
                    };

                    let session_id = generate_session_id();
                    let session_span = tracing::info_span!(
                        parent: None,
                        "relay.session",
                        session_id = session_id,
                        session_peer = session_peer,
                        relay_hostname = %relay_hostname,
                        app_id = tracing::field::Empty,
                        peer_relay_id = tracing::field::Empty,
                    );
                    let intake = intake.clone();
                    tokio::spawn(async move {
                        intake
                            .establish(session_id, connecting, session_span.clone())
                            .instrument(session_span)
                            .await;
                    });
                }
                // Arrivals are refused at the transport until the sessions
                // already accepted have all closed.
                tokio::select! {
                    _ = async {
                        while endpoint.accept().await.is_ok() {}
                    } => {}
                    _ = endpoint.wait_idle() => {}
                }
            })?;
        Ok(Self {
            join_handle,
            stop_sender: Some(stop_sender),
            #[cfg(test)]
            local_port,
        })
    }

    /// Ends the accept loop and refuses every later connection attempt at the
    /// transport; the sessions already accepted keep running.
    pub fn stop_accepting(&mut self) {
        if let Some(stop_sender) = self.stop_sender.take() {
            let _ = stop_sender.send(());
        }
    }

    /// Resolves once the accept loop has stopped and every transport
    /// connection of the endpoint has finished closing, so the process can
    /// exit without losing the CONNECTION_CLOSE of a session closed last.
    /// Call `stop_accepting` first; otherwise this never resolves.
    pub async fn wait_idle(&mut self) {
        let _ = (&mut self.join_handle).await;
    }
}

impl Drop for SessionHandler {
    fn drop(&mut self) {
        tracing::debug!("session handler dropped");
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use moqt::QUIC;

    use opentelemetry::global;
    use opentelemetry::trace::{TraceContextExt as _, TracerProvider as _};
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing::Span;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::layer::SubscriberExt;

    use super::session_setup_span;
    use crate::modules::{
        auth::{
            test_support::{
                client_endpoint, spawn_relay_and_connect_client, spawn_relay_with_verifier,
            },
            verified_token::VerifiedToken,
        },
        domain::session_peer::SessionPeer,
        test_support::{closed_spans::ClosedSpans, event_spans::EventSpans},
    };

    const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn a_client_setup_carrying_a_traceparent_continues_the_client_s_trace() {
        // Arrange
        global::set_text_map_propagator(TraceContextPropagator::new());
        let provider = SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        let _default = telemetry::testing::set_default(subscriber);
        let trace_context = moqt::TraceContext::parse(TRACEPARENT.as_bytes());

        // Act
        let span = session_setup_span(
            1,
            SessionPeer::Client,
            &Span::none(),
            trace_context.as_ref(),
        );

        // Assert
        let trace_id = span.context().span().span_context().trace_id();
        assert_eq!(trace_id.to_string(), "4bf92f3577b34da6a3ce929d0e0e4736");
    }

    #[tokio::test]
    async fn the_setup_span_closes_while_the_session_lives() {
        // Arrange
        let closed_spans = ClosedSpans::default();
        let _default = telemetry::testing::set_default(
            tracing_subscriber::registry().with(closed_spans.clone()),
        );

        // Act
        let (_relay, _session) = spawn_relay_and_connect_client(VerifiedToken::anonymous()).await;

        // Assert
        assert!(
            closed_spans.contains("relay.session.setup"),
            "closed spans: {:?}",
            closed_spans.names()
        );
    }

    #[tokio::test]
    async fn server_setup_is_sent_inside_the_setup_span() {
        // Arrange
        let event_spans = EventSpans::default();
        let _default = telemetry::testing::set_default(
            tracing_subscriber::registry().with(event_spans.clone()),
        );

        // Act
        let (_relay, _session) = spawn_relay_and_connect_client(VerifiedToken::anonymous()).await;

        // Assert
        assert_eq!(
            event_spans.spans_of("SERVER_SETUP sent"),
            [Some("relay.session.setup")]
        );
    }

    #[tokio::test]
    async fn spawning_on_a_port_in_use_returns_an_error() {
        // Arrange
        let relay = spawn_relay_with_verifier(VerifiedToken::anonymous()).await;

        // Act
        let result = relay.server.spawn_client_transport::<QUIC>(relay.port);

        // Assert
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_client_setup_does_not_block_new_connections() {
        // Arrange: client A finishes the QUIC handshake but never sends CLIENT_SETUP
        let relay = spawn_relay_with_verifier(VerifiedToken::anonymous()).await;
        let url = format!("moqt://127.0.0.1:{}", relay.port);
        let stalled_endpoint = client_endpoint(None);
        let _stalled_setup = stalled_endpoint.connect(&url).await.unwrap();

        // Act
        let endpoint = client_endpoint(None);
        let session = tokio::time::timeout(Duration::from_secs(2), async {
            endpoint.connect(&url).await?.await
        })
        .await;

        // Assert
        assert!(
            session.is_ok_and(|result| result.is_ok()),
            "a stalled CLIENT_SETUP blocked the accept loop"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stopped_handler_refuses_new_connections_at_once() {
        // Arrange: one session keeps the endpoint alive while accepting stops
        let (mut relay, _established) =
            spawn_relay_and_connect_client(VerifiedToken::anonymous()).await;
        let url = format!("moqt://127.0.0.1:{}", relay.port);
        relay.handler.stop_accepting();

        // Act
        let endpoint = client_endpoint(None);
        let connect = tokio::time::timeout(Duration::from_secs(2), async {
            endpoint.connect(&url).await?.await
        })
        .await;

        // Assert
        let result = connect.expect("the refusal must arrive within the timeout");
        assert!(result.is_err());
    }
}
