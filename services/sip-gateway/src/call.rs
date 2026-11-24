mod peer_audio;
mod peer_control;
mod phone_audio;
mod state_track;

use std::{net::IpAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use broadcast::{Broadcast, BroadcastWriter, RelayConnection, SourceTracks};
use moqt::{CertificateVerification, ClientConfig, client};
use rsipstack::{
    dialog::{
        dialog::{DialogState, DialogStateReceiver, TransactionHandle},
        invitation::InviteAsyncResult,
        invite_dialog::InviteDialog,
    },
    sip::{Header, Request, StatusCode, StatusCodeKind},
};
use tokio::{
    sync::{mpsc, watch},
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::{
    calls::CallSlot,
    media::rtp_leg::{Negotiated, RtpLeg},
    sip_trunk::{
        InboundInvite, Progress, Refresher, SessionTimer, SipTrunk, progress, sdp_content_type,
    },
};
use peer_audio::{PeerAudio, PeerWatchTask, PlayoutTask};
use peer_control::Command;
use phone_audio::PhoneAudioTask;
use state_track::StateTrack;
pub(crate) use state_track::{CallState, EndReason};

/// How long the trunk may take to answer an INVITE.
pub(crate) const RINGING_TIMEOUT: Duration = Duration::from_secs(60);
/// A call whose trunk sends no audio this long has lost its media path. Some
/// trunks hang up on RTP inactivity as well; the gateway does not wait for them.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the broadcast stays after `ended`, so a page whose
/// `PhoneCall.join` finishes after the call already ended (a busy phone's
/// immediate 486) still reads why it ended.
const ENDED_LINGER: Duration = Duration::from_secs(5);
/// A refresh answered 491 (both sides re-INVITE at once) is sent this many
/// times at most.
const REFRESH_ATTEMPTS: usize = 3;

pub(crate) struct CallContext {
    pub(crate) trunk: SipTrunk,
    pub(crate) relay_url: String,
    pub(crate) verify_relay_certificate: bool,
    /// The address the trunk sends media to, when it is not the bound one.
    pub(crate) media_address: Option<IpAddr>,
    pub(crate) max_duration: Duration,
}

/// One call, from its INVITE to its end.
pub(crate) struct CallPlan {
    pub(crate) slot: CallSlot,
    pub(crate) app_id: String,
    pub(crate) direction: Direction,
    /// `<room>/phone/<call_id>`, the broadcast the gateway publishes.
    pub(crate) namespace: Vec<String>,
    /// The broadcast the page publishes for this call.
    pub(crate) peer: Vec<String>,
    pub(crate) relay_token: Option<String>,
}

pub(crate) enum Direction {
    /// The app server asked for a call to `to`, showing `from` as the caller:
    /// the app's number, or a user name.
    Outbound { from: String, to: String },
    /// The trunk's INVITE to one of the app's numbers, which the app's call
    /// webhook answered.
    Inbound(AnsweringInvite),
}

/// An inbound INVITE whose SDP answer is built but not sent yet.
pub(crate) struct AnsweringInvite {
    pub(crate) invite: InboundInvite,
    pub(crate) leg: RtpLeg,
    pub(crate) answer: String,
    pub(crate) negotiated: Negotiated,
}

impl Direction {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Outbound { .. } => "outbound",
            Self::Inbound(_) => "inbound",
        }
    }
}

/// Cancelling the slot's `hang_up` ends the call with BYE, or CANCEL while
/// it rings.
pub(crate) async fn run(context: Arc<CallContext>, plan: CallPlan) {
    let direction = plan.direction.label();
    let span = tracing::info_span!(
        "sip_gateway.call",
        call_id = %plan.slot.call_id,
        app_id = %plan.app_id,
        direction,
    );
    let call_id = plan.slot.call_id.clone();
    let hang_up = plan.slot.hang_up.clone();
    let app_id = plan.app_id.clone();
    let unanswered = match &plan.direction {
        Direction::Inbound(answering) => Some(answering.invite.dialog.clone()),
        Direction::Outbound { .. } => None,
    };
    let reason = async {
        match Call::start(&context, plan).await {
            Ok(call) => call.run(&context, hang_up).await,
            Err(error) => {
                tracing::error!(error = %format!("{error:#}"), call_id, app_id, "call not started");
                if let Some(dialog) = unanswered {
                    reject(&dialog, StatusCode::ServiceUnavailable);
                }
                EndReason::Gateway
            }
        }
    }
    .instrument(span)
    .await;
    tracing::info!(
        call_id,
        app_id,
        direction,
        call_end_reason = reason.label(),
        "call ended"
    );
}

struct Call {
    call_id: String,
    slot: CallSlot,
    session: Arc<client::Session>,
    broadcast: Broadcast,
    state_track: StateTrack,
    leg: Arc<RtpLeg>,
    dialog: InviteDialog,
    states: DialogStateReceiver,
    setup: Setup,
    /// The gateway's SDP: its offer, or its answer to the trunk's. A refresh
    /// sends it again unchanged.
    local_sdp: String,
    /// Settled by the trunk's 2xx to the gateway's INVITE, or by the trunk's INVITE.
    timer: Option<SessionTimer>,
    peer_watch: PeerWatchTask,
    peer_audio: Option<mpsc::Receiver<PeerAudio>>,
    peer_commands: mpsc::Receiver<Command>,
}

/// What is left of the INVITE once it is sent or answered.
enum Setup {
    /// The trunk's final response, once it ends the dialog without a 2xx.
    Outbound {
        result: rsipstack::platform::JoinHandle<InviteAsyncResult>,
    },
    /// Settled by the gateway's own answer.
    Inbound { negotiated: Negotiated },
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
        let (peer_watch, peer_audio, peer_commands) =
            PeerWatchTask::run(session.clone(), plan.peer.clone());
        let call_id = plan.slot.call_id.clone();
        let (leg, dialog, states, setup, local_sdp, timer) = match plan.direction {
            Direction::Outbound { from, to } => {
                state_track.publish(&CallState::Dialing).await?;
                let (leg, offer) = RtpLeg::offer(context.media_address).await?;
                let invite = context.trunk.invite(&call_id, &from, &to, offer.clone())?;
                tracing::info!(
                    call_id,
                    sip_call_id = invite.dialog.id().call_id,
                    "INVITE sent"
                );
                let setup = Setup::Outbound {
                    result: invite.result,
                };
                (leg, invite.dialog, invite.states, setup, offer, None)
            }
            Direction::Inbound(answering) => {
                state_track.publish(&CallState::Ringing).await?;
                let dialog = answering.invite.dialog;
                let timer = answering.invite.timer;
                let mut headers = sdp_content_type();
                if let Some(timer) = timer {
                    headers.extend([
                        timer.session_expires(false),
                        Header::Require("timer".into()),
                    ]);
                }
                dialog
                    .accept(Some(headers), Some(answering.answer.clone().into_bytes()))
                    .context("answer the INVITE")?;
                tracing::info!(
                    call_id,
                    sip_call_id = dialog.id().call_id,
                    sip_status = 200,
                    "INVITE final response sent"
                );
                let setup = Setup::Inbound {
                    negotiated: answering.negotiated,
                };
                (
                    answering.leg,
                    dialog,
                    answering.invite.states,
                    setup,
                    answering.answer,
                    timer,
                )
            }
        };
        Ok(Self {
            call_id,
            slot: plan.slot,
            session,
            broadcast,
            state_track,
            leg: Arc::new(leg),
            dialog,
            states,
            setup,
            local_sdp,
            timer,
            peer_watch,
            peer_audio: Some(peer_audio),
            peer_commands,
        })
    }

    async fn run(mut self, context: &CallContext, hang_up: CancellationToken) -> EndReason {
        let reason = match self.until_answered(&hang_up).await {
            Ok(negotiated) => self.bridge(context, &hang_up, negotiated).await,
            Err(reason) => reason,
        };
        self.end(context, &reason).await;
        reason
    }

    /// The media an outbound call's 2xx or an inbound call's ACK settled, or
    /// why the call ended before.
    async fn until_answered(
        &mut self,
        hang_up: &CancellationToken,
    ) -> Result<Negotiated, EndReason> {
        let deadline = Instant::now() + RINGING_TIMEOUT;
        loop {
            tokio::select! {
                state = self.states.recv() => {
                    let Some(state) = state else {
                        return Err(EndReason::Gateway);
                    };
                    match progress(&state) {
                        Progress::Ringing(sip_status) if matches!(self.setup, Setup::Outbound { .. }) => {
                            tracing::info!(call_id = %self.call_id, sip_status, "INVITE provisional response received");
                            self.publish(&CallState::Ringing).await;
                        }
                        Progress::Answered { answer, timer } => return self.answered(&answer, timer).await,
                        Progress::Ended => return Err(self.ended_unanswered().await),
                        Progress::Ringing(_) | Progress::Other => {}
                    }
                }
                () = sleep_until(deadline) => {
                    hang_up_dialog(&self.dialog).await;
                    return Err(EndReason::Timeout);
                }
                () = hang_up.cancelled() => {
                    hang_up_dialog(&self.dialog).await;
                    return Err(EndReason::Gateway);
                }
                Some(Command::Hangup) = self.peer_commands.recv() => {
                    tracing::info!(call_id = %self.call_id, "the peer hung up");
                    hang_up_dialog(&self.dialog).await;
                    return Err(EndReason::Hangup);
                }
            }
        }
    }

    /// Applies the trunk's SDP answer and the session timer of its 2xx to an
    /// outbound call; an inbound call's media was settled by the gateway's
    /// own answer.
    async fn answered(
        &mut self,
        answer: &str,
        timer: Option<SessionTimer>,
    ) -> Result<Negotiated, EndReason> {
        match &self.setup {
            Setup::Inbound { negotiated } => {
                tracing::info!(call_id = %self.call_id, "ACK received");
                Ok(*negotiated)
            }
            Setup::Outbound { .. } => {
                tracing::info!(call_id = %self.call_id, sip_status = 200, "INVITE final response received");
                self.timer = timer;
                match self.leg.answer(answer).await {
                    Ok(negotiated) => Ok(negotiated),
                    Err(error) => {
                        tracing::warn!(%error, call_id = %self.call_id, "SDP answer refused");
                        hang_up_dialog(&self.dialog).await;
                        Err(EndReason::Status(488))
                    }
                }
            }
        }
    }

    async fn bridge(
        &mut self,
        context: &CallContext,
        hang_up: &CancellationToken,
        negotiated: Negotiated,
    ) -> EndReason {
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
        let (digit_sender, digits) = mpsc::unbounded_channel();
        let _playout = PlayoutTask::run(
            self.call_id.clone(),
            negotiated,
            self.leg.sender(negotiated.codec),
            peer_audio,
            digits,
        );
        let limit = Instant::now() + context.max_duration;
        let next_refresh =
            |timer: Option<SessionTimer>| timer.map(|timer| Instant::now() + timer.wait());
        let mut refresh_at = next_refresh(self.timer);
        loop {
            let silent_until = *last_audio.borrow() + SILENCE_TIMEOUT;
            tokio::select! {
                state = self.states.recv() => match state {
                    Some(DialogState::Updated(_, request, handle)) => {
                        self.answer_refresh(&request, handle).await;
                        refresh_at = next_refresh(self.timer);
                    }
                    state => if matches!(state.as_ref().map(progress), Some(Progress::Ended) | None) {
                        tracing::info!(call_id = %self.call_id, "BYE received");
                        return EndReason::Hangup;
                    },
                },
                () = sleep_until(refresh_at.unwrap_or(limit)), if refresh_at.is_some() => {
                    if !self.refresh().await {
                        hang_up_dialog(&self.dialog).await;
                        return EndReason::Timeout;
                    }
                    refresh_at = next_refresh(self.timer);
                }
                () = sleep_until(silent_until) => {
                    if *last_audio.borrow() + SILENCE_TIMEOUT <= Instant::now() {
                        hang_up_dialog(&self.dialog).await;
                        return EndReason::Timeout;
                    }
                }
                () = sleep_until(limit) => {
                    hang_up_dialog(&self.dialog).await;
                    return EndReason::Limit;
                }
                () = hang_up.cancelled() => {
                    hang_up_dialog(&self.dialog).await;
                    return EndReason::Gateway;
                }
                Some(command) = self.peer_commands.recv() => match command {
                    Command::Hangup => {
                        tracing::info!(call_id = %self.call_id, "the peer hung up");
                        hang_up_dialog(&self.dialog).await;
                        return EndReason::Hangup;
                    }
                    Command::Dtmf { digit } => {
                        let _ = digit_sender.send(digit);
                    }
                    Command::Other => {}
                },
            }
        }
    }

    /// Answers the trunk's re-INVITE or UPDATE, which refreshes the session
    /// (RFC 4028): with the gateway's SDP again when it carries an offer.
    async fn answer_refresh(&self, request: &Request, handle: TransactionHandle) {
        tracing::info!(call_id = %self.call_id, "session refresh received");
        let mut headers = Vec::new();
        let mut body = None;
        if !request.body.is_empty() {
            headers.extend(sdp_content_type());
            body = Some(self.local_sdp.clone().into_bytes());
        }
        if let Some(timer) = self.timer {
            headers.extend([
                timer.session_expires(false),
                Header::Require("timer".into()),
            ]);
        }
        if let Err(error) = handle.respond(StatusCode::OK, Some(headers), body).await {
            tracing::warn!(%error, call_id = %self.call_id, "session refresh not answered");
        }
    }

    /// Acts when the session timer fires: as the refresher, re-INVITEs with
    /// the same SDP (again after a 491, RFC 3261 §14.1); otherwise the trunk
    /// let the session expire. `false` when the call must end.
    async fn refresh(&self) -> bool {
        let Some(timer) = self
            .timer
            .filter(|timer| timer.refresher == Refresher::Gateway)
        else {
            tracing::warn!(call_id = %self.call_id, "session expired without a refresh");
            return false;
        };
        let mut headers = sdp_content_type();
        headers.push(timer.session_expires(true));
        let refreshed = self
            .dialog
            .reinvite_with_glare_retry(
                Some(headers),
                Some(self.local_sdp.clone().into_bytes()),
                REFRESH_ATTEMPTS,
            )
            .await;
        match refreshed {
            Ok(Some(response)) if response.status_code.kind() == StatusCodeKind::Successful => {
                tracing::info!(call_id = %self.call_id, "session refreshed");
                true
            }
            Ok(response) => {
                let sip_status = response.map(|response| response.status_code.code());
                tracing::warn!(call_id = %self.call_id, ?sip_status, "session refresh refused");
                false
            }
            Err(error) => {
                tracing::warn!(%error, call_id = %self.call_id, "session refresh failed");
                false
            }
        }
    }

    /// Why the dialog ended before it was answered: for an outbound call
    /// the trunk's final response, for an inbound one a CANCEL or BYE.
    async fn ended_unanswered(&mut self) -> EndReason {
        let Setup::Outbound { result } = &mut self.setup else {
            tracing::info!(call_id = %self.call_id, "CANCEL received");
            return EndReason::Hangup;
        };
        match result.await {
            Ok(Ok((_, Some(response)))) => {
                let sip_status = response.status_code.code();
                tracing::info!(call_id = %self.call_id, sip_status, "INVITE final response received");
                EndReason::Status(sip_status)
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, call_id = %self.call_id, "INVITE failed");
                EndReason::Gateway
            }
            _ => EndReason::Gateway,
        }
    }

    async fn end(self, context: &CallContext, reason: &EndReason) {
        self.publish(&CallState::Ended(reason.clone())).await;
        self.leg.close();
        context.trunk.remove_dialog(&self.dialog.id());
        let hang_up = self.slot.hang_up.clone();
        drop(self.slot);
        drop(self.peer_watch);
        tokio::select! {
            () = tokio::time::sleep(ENDED_LINGER) => {}
            () = hang_up.cancelled() => {}
        }
        self.broadcast.connection().close().await;
        self.session.close().await;
    }

    async fn publish(&self, state: &CallState) {
        if let Err(error) = self.state_track.publish(state).await {
            tracing::warn!(%error, call_id = %self.call_id, "call state not published");
        }
    }
}

/// Refuses an inbound INVITE not answered yet.
pub(crate) fn reject(dialog: &InviteDialog, status: StatusCode) {
    let sip_call_id = dialog.id().call_id;
    let sip_status = status.code();
    tracing::info!(sip_call_id, sip_status, "INVITE final response sent");
    if let Err(error) = dialog.reject(Some(status), None) {
        tracing::warn!(sip_call_id, %error, "INVITE final response not sent");
    }
}

/// BYE, or CANCEL while an outbound INVITE is unanswered.
async fn hang_up_dialog(dialog: &InviteDialog) {
    let sip_call_id = dialog.id().call_id;
    tracing::info!(sip_call_id, "hangup sent");
    if let Err(error) = dialog.hangup().await {
        tracing::warn!(sip_call_id, %error, "hangup not sent");
    }
}
