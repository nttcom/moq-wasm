mod peer_audio;
mod phone_audio;
mod state_track;

use std::{net::IpAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use broadcast::{Broadcast, BroadcastWriter, RelayConnection, SourceTracks};
use moqt::{CertificateVerification, ClientConfig, client};
use rsipstack::dialog::invite_dialog::InviteDialog;
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::{
    media::rtp_leg::RtpLeg,
    sip_trunk::{OutboundInvite, Progress, SipTrunk, progress},
};
use peer_audio::{PeerAudio, PeerWatchTask, PlayoutTask};
use phone_audio::PhoneAudioTask;
use state_track::StateTrack;
pub(crate) use state_track::{CallState, EndReason};

/// How long the trunk may take to answer an INVITE.
pub(crate) const RINGING_TIMEOUT: Duration = Duration::from_secs(60);
/// A call whose trunk sends no audio this long has lost its media path. Some
/// trunks hang up on RTP inactivity as well; the gateway does not wait for them.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct CallContext {
    pub(crate) trunk: SipTrunk,
    pub(crate) relay_url: String,
    pub(crate) verify_relay_certificate: bool,
    /// The address the trunk sends media to, when it is not the bound one.
    pub(crate) media_address: Option<IpAddr>,
    pub(crate) max_duration: Duration,
}

/// One outbound call the app server asked for.
pub(crate) struct CallPlan {
    pub(crate) call_id: String,
    pub(crate) app_id: String,
    /// The caller the INVITE shows: the app's number, or a user name.
    pub(crate) from: String,
    pub(crate) to: String,
    /// `<room>/phone/<call_id>`, the broadcast the gateway publishes.
    pub(crate) namespace: Vec<String>,
    /// The broadcast the page publishes for this call.
    pub(crate) peer: Vec<String>,
    pub(crate) relay_token: Option<String>,
}

/// Cancelling `hang_up` ends the call with BYE, or CANCEL while it rings.
pub(crate) async fn run(context: Arc<CallContext>, plan: CallPlan, hang_up: CancellationToken) {
    let span = tracing::info_span!(
        "sip_gateway.call",
        call_id = %plan.call_id,
        app_id = %plan.app_id,
        direction = "outbound",
    );
    let call_id = plan.call_id.clone();
    let app_id = plan.app_id.clone();
    let reason = async {
        match Call::start(&context, plan).await {
            Ok(call) => call.run(&context, hang_up).await,
            Err(error) => {
                tracing::error!(error = %format!("{error:#}"), call_id, app_id, "call not started");
                EndReason::Gateway
            }
        }
    }
    .instrument(span)
    .await;
    tracing::info!(
        call_id,
        app_id,
        call_end_reason = reason.label(),
        "call ended"
    );
}

struct Call {
    plan: CallPlan,
    session: Arc<client::Session>,
    broadcast: Broadcast,
    state_track: StateTrack,
    leg: Arc<RtpLeg>,
    invite: OutboundInvite,
    peer_watch: PeerWatchTask,
    peer_audio: Option<mpsc::Receiver<PeerAudio>>,
    peer_left: oneshot::Receiver<()>,
}

impl Call {
    async fn start(context: &CallContext, plan: CallPlan) -> Result<Self> {
        let session = Arc::new(
            client::Session::connect(
                &context.relay_url,
                &ClientConfig {
                    certificate_verification: CertificateVerification::native_roots_or_disabled(
                        context.verify_relay_certificate,
                    ),
                    authorization_token: plan.relay_token.clone().map(Into::into),
                    ..ClientConfig::default()
                },
            )
            .await
            .context("connect to the relay")?,
        );
        let connection = RelayConnection::with_client_session(&session);
        session.add_handler(Arc::new(connection.clone()));
        let broadcast = Broadcast::new(connection, plan.namespace.clone());
        let state_track = StateTrack::open(broadcast.clone()).await?;
        state_track.publish(&CallState::Dialing).await?;
        let (peer_watch, peer_audio, peer_left) =
            PeerWatchTask::run(session.clone(), plan.peer.clone());
        let (leg, offer) = RtpLeg::offer(context.media_address).await?;
        let invite = context
            .trunk
            .invite(&plan.call_id, &plan.from, &plan.to, offer)?;
        tracing::info!(
            call_id = %plan.call_id,
            sip_call_id = invite.dialog.id().call_id,
            "INVITE sent"
        );
        Ok(Self {
            plan,
            session,
            broadcast,
            state_track,
            leg: Arc::new(leg),
            invite,
            peer_watch,
            peer_audio: Some(peer_audio),
            peer_left,
        })
    }

    async fn run(mut self, context: &CallContext, hang_up: CancellationToken) -> EndReason {
        let reason = match self.until_answered(&hang_up).await {
            Ok(answer) => self.bridge(context, &hang_up, &answer).await,
            Err(reason) => reason,
        };
        self.end(context, &reason).await;
        reason
    }

    /// The SDP answer of the 2xx, or why the call ended before one.
    async fn until_answered(&mut self, hang_up: &CancellationToken) -> Result<String, EndReason> {
        let deadline = Instant::now() + RINGING_TIMEOUT;
        loop {
            tokio::select! {
                state = self.invite.states.recv() => {
                    let Some(state) = state else {
                        return Err(EndReason::Gateway);
                    };
                    match progress(&state) {
                        Progress::Ringing(sip_status) => {
                            tracing::info!(call_id = %self.plan.call_id, sip_status, "INVITE provisional response received");
                            self.publish(&CallState::Ringing).await;
                        }
                        Progress::Answered(answer) => {
                            tracing::info!(call_id = %self.plan.call_id, sip_status = 200, "INVITE final response received");
                            return Ok(answer);
                        }
                        Progress::Ended => return Err(self.final_status().await),
                        Progress::Other => {}
                    }
                }
                () = sleep_until(deadline) => {
                    hang_up_dialog(&self.invite.dialog).await;
                    return Err(EndReason::Timeout);
                }
                () = hang_up.cancelled() => {
                    hang_up_dialog(&self.invite.dialog).await;
                    return Err(EndReason::Gateway);
                }
                Ok(()) = &mut self.peer_left => {
                    hang_up_dialog(&self.invite.dialog).await;
                    return Err(EndReason::Hangup);
                }
            }
        }
    }

    async fn bridge(
        &mut self,
        context: &CallContext,
        hang_up: &CancellationToken,
        answer: &str,
    ) -> EndReason {
        let negotiated = match self.leg.answer(answer).await {
            Ok(negotiated) => negotiated,
            Err(error) => {
                tracing::warn!(%error, call_id = %self.plan.call_id, "SDP answer refused");
                hang_up_dialog(&self.invite.dialog).await;
                return EndReason::Status(488);
            }
        };
        self.publish(&CallState::Answered).await;
        let writer = BroadcastWriter::run(
            self.broadcast.clone(),
            SourceTracks {
                cmaf: false,
                ..SourceTracks::default()
            },
        );
        let (last_audio_sender, last_audio) = watch::channel(Instant::now());
        let _phone_audio = PhoneAudioTask::run(
            self.leg.clone(),
            negotiated.codec,
            writer,
            last_audio_sender,
        );
        let peer_audio = self.peer_audio.take().expect("a call is bridged once");
        let _playout = PlayoutTask::run(
            negotiated.codec,
            self.leg.sender(negotiated.codec),
            peer_audio,
        );
        let limit = Instant::now() + context.max_duration;
        loop {
            let silent_until = *last_audio.borrow() + SILENCE_TIMEOUT;
            tokio::select! {
                state = self.invite.states.recv() => match state.as_ref().map(progress) {
                    Some(Progress::Ended) | None => {
                        tracing::info!(call_id = %self.plan.call_id, "BYE received");
                        return EndReason::Hangup;
                    }
                    _ => {}
                },
                () = sleep_until(silent_until) => {
                    if *last_audio.borrow() + SILENCE_TIMEOUT <= Instant::now() {
                        hang_up_dialog(&self.invite.dialog).await;
                        return EndReason::Timeout;
                    }
                }
                () = sleep_until(limit) => {
                    hang_up_dialog(&self.invite.dialog).await;
                    return EndReason::Limit;
                }
                () = hang_up.cancelled() => {
                    hang_up_dialog(&self.invite.dialog).await;
                    return EndReason::Gateway;
                }
                Ok(()) = &mut self.peer_left => {
                    hang_up_dialog(&self.invite.dialog).await;
                    return EndReason::Hangup;
                }
            }
        }
    }

    /// The final response to the INVITE, once the dialog ended without a 2xx.
    async fn final_status(&mut self) -> EndReason {
        match (&mut self.invite.result).await {
            Ok(Ok((_, Some(response)))) => {
                let sip_status = response.status_code.code();
                tracing::info!(call_id = %self.plan.call_id, sip_status, "INVITE final response received");
                EndReason::Status(sip_status)
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, call_id = %self.plan.call_id, "INVITE failed");
                EndReason::Gateway
            }
            _ => EndReason::Gateway,
        }
    }

    async fn end(self, context: &CallContext, reason: &EndReason) {
        self.publish(&CallState::Ended(reason.clone())).await;
        self.leg.close();
        context.trunk.remove_dialog(&self.invite.dialog.id());
        drop(self.peer_watch);
        self.broadcast.connection().close().await;
        self.session.close().await;
    }

    async fn publish(&self, state: &CallState) {
        if let Err(error) = self.state_track.publish(state).await {
            tracing::warn!(%error, call_id = %self.plan.call_id, "call state not published");
        }
    }
}

/// BYE, or CANCEL while the INVITE is unanswered.
async fn hang_up_dialog(dialog: &InviteDialog) {
    let call_id = dialog.id().call_id;
    tracing::info!(call_id, "hangup sent");
    if let Err(error) = dialog.hangup().await {
        tracing::warn!(call_id, %error, "hangup not sent");
    }
}
