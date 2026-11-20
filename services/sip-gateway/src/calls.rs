use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use app_registry::{App, RegistryStore};
use auth_token::{Claims, sign_token, unix_now};
use rsipstack::{
    dialog::dialog::{DialogState, DialogStateReceiver},
    sip::StatusCode,
};
use serde::Deserialize;
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    call::{self, AnsweringInvite, CallContext, CallPlan, Direction, RINGING_TIMEOUT, reject},
    call_auth::AuthorizedCall,
    media::rtp_leg::RtpLeg,
    sip_trunk::InboundInvite,
    webhook::{CallWebhook, Decision, IncomingCall},
};

/// The caller an INVITE shows for an app without a number of its own.
const ANONYMOUS_CALLER: &str = "anonymous";
/// The relay token outlives the longest call by this much.
const RELAY_TOKEN_MARGIN: Duration = Duration::from_secs(5 * 60);

/// The calls in progress, at most `max_calls_per_app` for each app.
pub(crate) struct Calls {
    context: Arc<CallContext>,
    max_calls_per_app: usize,
    /// The apps' numbers and call webhooks; `None` refuses every INVITE
    /// from the trunk with 404.
    registry: Option<Arc<RegistryStore>>,
    webhook: CallWebhook,
    active: Arc<Mutex<HashMap<String, ActiveCall>>>,
    tasks: TaskTracker,
}

struct ActiveCall {
    app_id: String,
    hang_up: CancellationToken,
}

/// A call counted against its app's limit until it is dropped, which the
/// call does once it has ended, before it leaves the relay.
pub(crate) struct CallSlot {
    pub(crate) call_id: String,
    /// Cancelled to end the call (SIGTERM).
    pub(crate) hang_up: CancellationToken,
    active: Arc<Mutex<HashMap<String, ActiveCall>>>,
}

impl Drop for CallSlot {
    fn drop(&mut self) {
        self.active
            .lock()
            .expect("calls lock")
            .remove(&self.call_id);
    }
}

/// An app server's request for an outbound call (`POST /calls`).
#[derive(Deserialize)]
pub(crate) struct CallRequest {
    /// The room, `<app>/...`.
    pub(crate) namespace_prefix: String,
    /// The broadcast the page publishes for this call, under the room.
    pub(crate) peer: String,
    pub(crate) to: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PlacedCall {
    pub(crate) call_id: String,
    /// `<room>/phone/<call_id>`, the broadcast the gateway publishes.
    pub(crate) namespace: String,
}

#[derive(Debug)]
pub(crate) enum PlaceError {
    /// The peer is not a broadcast of the room, or is the gateway's own.
    InvalidPeer,
    /// The app has `max_calls_per_app` calls in progress.
    Busy,
    Failed(anyhow::Error),
}

impl Calls {
    pub(crate) fn new(
        context: CallContext,
        max_calls_per_app: usize,
        registry: Option<Arc<RegistryStore>>,
    ) -> Self {
        Self {
            context: Arc::new(context),
            max_calls_per_app,
            registry,
            webhook: CallWebhook::default(),
            active: Arc::new(Mutex::new(HashMap::new())),
            tasks: TaskTracker::new(),
        }
    }

    /// Starts the call and returns at once; the call reports its progress
    /// on its `call` track.
    pub(crate) fn place(
        &self,
        authorized: AuthorizedCall,
        request: CallRequest,
    ) -> Result<PlacedCall, PlaceError> {
        let reservation = self.reserve(&authorized.app_id).ok_or(PlaceError::Busy)?;
        let direction = Direction::Outbound {
            from: authorized
                .from
                .unwrap_or_else(|| ANONYMOUS_CALLER.to_string()),
            to: request.to,
        };
        let plan = self.plan(
            reservation,
            authorized.app_id,
            authorized.secret.as_deref(),
            &request.namespace_prefix,
            &request.peer,
            direction,
        )?;
        let placed = PlacedCall {
            call_id: plan.slot.call_id.clone(),
            namespace: plan.namespace.join("/"),
        };
        let context = self.context.clone();
        self.tasks.spawn(call::run(context, plan));
        Ok(placed)
    }

    /// Answers an INVITE the trunk sent: rings, asks the webhook of the app
    /// the number belongs to, and bridges the call into the room it names.
    async fn answer(&self, mut invite: InboundInvite) {
        let Some(registry) = &self.registry else {
            return reject(&invite.dialog, StatusCode::NotFound);
        };
        let Ok(apps) = registry.load().await else {
            return reject(&invite.dialog, StatusCode::ServiceUnavailable);
        };
        let Some((app, webhook_url)) = apps.iter().find_map(|app| {
            let phone = app.phone.as_ref()?;
            phone
                .numbers
                .contains(&invite.to)
                .then_some((app, phone.call_webhook.clone()?))
        }) else {
            tracing::info!(
                sip_call_id = invite.dialog.id().call_id,
                "INVITE to an unknown number"
            );
            return reject(&invite.dialog, StatusCode::NotFound);
        };
        let Some(reservation) = self.reserve(&app.app_id) else {
            return reject(&invite.dialog, StatusCode::BusyHere);
        };
        let (leg, answer, negotiated) =
            match RtpLeg::answer_offer(&invite.offer, self.context.media_address).await {
                Ok(answered) => answered,
                Err(error) => {
                    tracing::warn!(%error, call_id = reservation.call_id, "SDP offer refused");
                    return reject(&invite.dialog, StatusCode::NotAcceptableHere);
                }
            };
        if let Err(error) = invite.dialog.ringing(None, None) {
            tracing::warn!(%error, call_id = reservation.call_id, "180 not sent");
        }
        tracing::info!(
            call_id = reservation.call_id,
            app_id = app.app_id,
            sip_status = 180,
            "INVITE provisional response sent"
        );
        let call = IncomingCall {
            call_id: reservation.call_id.clone(),
            from: invite.from.clone(),
            to: invite.to.clone(),
        };
        let decision = webhook_decision(
            &self.webhook,
            &webhook_url,
            app,
            &call,
            &mut invite.states,
            &reservation.hang_up,
        )
        .await;
        let (namespace_prefix, peer) = match decision {
            Ok(room) => room,
            Err(status) => {
                leg.close();
                if let Some(status) = status {
                    reject(&invite.dialog, status);
                }
                return;
            }
        };
        let dialog = invite.dialog.clone();
        let direction = Direction::Inbound(AnsweringInvite {
            invite,
            leg,
            answer,
            negotiated,
        });
        let call_id = reservation.call_id.clone();
        let plan = match self.plan(
            reservation,
            app.app_id.clone(),
            Some(&app.secret),
            &namespace_prefix,
            &peer,
            direction,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                tracing::warn!(
                    ?error,
                    call_id,
                    app_id = app.app_id,
                    namespace_prefix,
                    peer,
                    "call webhook named no room of the app"
                );
                return reject(&dialog, StatusCode::ServerInternalError);
            }
        };
        tracing::info!(
            call_id = plan.slot.call_id,
            app_id = plan.app_id,
            direction = "inbound",
            "call answered"
        );
        call::run(self.context.clone(), plan).await;
    }

    /// Ends every call with BYE (or CANCEL) and waits until they have ended.
    pub(crate) async fn hang_up_all(&self) {
        for call in self.active.lock().expect("calls lock").values() {
            call.hang_up.cancel();
        }
        self.tasks.close();
        self.tasks.wait().await;
    }

    /// A new call of the app, unless it has its maximum in progress.
    fn reserve(&self, app_id: &str) -> Option<CallSlot> {
        let mut active = self.active.lock().expect("calls lock");
        let in_progress = active.values().filter(|call| call.app_id == app_id).count();
        if in_progress >= self.max_calls_per_app {
            return None;
        }
        let call_id = random_hex();
        let hang_up = CancellationToken::new();
        active.insert(
            call_id.clone(),
            ActiveCall {
                app_id: app_id.to_string(),
                hang_up: hang_up.clone(),
            },
        );
        Some(CallSlot {
            call_id,
            hang_up,
            active: self.active.clone(),
        })
    }

    /// The call bridged into the room `namespace_prefix` with `peer`, which
    /// joins the relay with a token signed with `secret` (none without
    /// authorization).
    fn plan(
        &self,
        slot: CallSlot,
        app_id: String,
        secret: Option<&str>,
        namespace_prefix: &str,
        peer: &str,
        direction: Direction,
    ) -> Result<CallPlan, PlaceError> {
        let peer_elements = peer_of_room(namespace_prefix, peer)?;
        let namespace = format!("{namespace_prefix}/phone/{}", slot.call_id);
        let relay_token = secret
            .map(|secret| {
                relay_token(
                    &app_id,
                    secret,
                    &namespace,
                    peer,
                    unix_now(),
                    self.context.max_duration + RELAY_TOKEN_MARGIN,
                )
            })
            .transpose()
            .map_err(PlaceError::Failed)?;
        Ok(CallPlan {
            slot,
            app_id,
            direction,
            namespace: split(&namespace),
            peer: peer_elements,
            relay_token,
        })
    }
}

pub(crate) struct InboundTask {
    join_handle: JoinHandle<()>,
}

impl InboundTask {
    pub(crate) fn run(
        calls: Arc<Calls>,
        mut invites: mpsc::UnboundedReceiver<InboundInvite>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            while let Some(invite) = invites.recv().await {
                let answering = calls.clone();
                calls
                    .tasks
                    .spawn(async move { answering.answer(invite).await });
            }
        });
        Self { join_handle }
    }
}

impl Drop for InboundTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

/// The room and the peer the app's webhook bridges the call into, or the
/// status that refuses the INVITE; `None` once the trunk cancelled it.
async fn webhook_decision(
    webhook: &CallWebhook,
    url: &str,
    app: &App,
    call: &IncomingCall,
    states: &mut DialogStateReceiver,
    hang_up: &CancellationToken,
) -> Result<(String, String), Option<StatusCode>> {
    let decision = tokio::select! {
        decision = tokio::time::timeout(
            RINGING_TIMEOUT,
            webhook.ask(url, &app.app_id, &app.secret, call, unix_now()),
        ) => decision,
        () = cancelled(states) => {
            tracing::info!(call_id = call.call_id, "CANCEL received");
            return Err(None);
        }
        () = hang_up.cancelled() => return Err(Some(StatusCode::ServiceUnavailable)),
    };
    match decision {
        Ok(Ok(Decision::Answer {
            namespace_prefix,
            peer,
        })) => Ok((namespace_prefix, peer)),
        Ok(Ok(Decision::Reject { reject: refusal })) => {
            tracing::info!(
                call_id = call.call_id,
                app_id = app.app_id,
                "call refused by the webhook"
            );
            Err(Some(StatusCode::from(refusal.sip_status())))
        }
        Ok(Err(error)) => {
            tracing::warn!(error = %format!("{error:#}"), call_id = call.call_id, app_id = app.app_id, "call webhook failed");
            Err(Some(StatusCode::TemporarilyUnavailable))
        }
        Err(_) => {
            tracing::warn!(
                call_id = call.call_id,
                app_id = app.app_id,
                "call webhook timed out"
            );
            Err(Some(StatusCode::TemporarilyUnavailable))
        }
    }
}

/// Resolves once the trunk ended the dialog (CANCEL) or it is gone.
async fn cancelled(states: &mut DialogStateReceiver) {
    while let Some(state) = states.recv().await {
        if matches!(state, DialogState::Terminated(..)) {
            return;
        }
    }
}

/// The peer's namespace elements, when it is a broadcast of the room other
/// than a call's.
fn peer_of_room(namespace_prefix: &str, peer: &str) -> Result<Vec<String>, PlaceError> {
    let relative = peer
        .strip_prefix(namespace_prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or(PlaceError::InvalidPeer)?;
    let elements = split(relative);
    if elements.iter().any(String::is_empty) || elements.first().is_some_and(|e| e == "phone") {
        return Err(PlaceError::InvalidPeer);
    }
    Ok(split(peer))
}

/// The client token the gateway joins the relay with for one call: it
/// publishes only the call's broadcast and subscribes only to the peer.
fn relay_token(
    app_id: &str,
    secret: &str,
    namespace: &str,
    peer: &str,
    now: u64,
    lifetime: Duration,
) -> Result<String> {
    let relative = |path: &str| {
        path.strip_prefix(app_id)
            .and_then(|rest| rest.strip_prefix('/'))
            .map(str::to_string)
            .with_context(|| format!("{path} is not a namespace of {app_id}"))
    };
    let claims = Claims::issued(
        app_id.to_string(),
        Some(relative(namespace)?),
        Some(relative(peer)?),
        now,
        lifetime,
    );
    sign_token(&claims, secret)
}

fn split(namespace: &str) -> Vec<String> {
    namespace.split('/').map(str::to_string).collect()
}

fn random_hex() -> String {
    let mut bytes = [0u8; 16];
    aws_lc_rs::rand::fill(&mut bytes).expect("the system random number generator works");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use auth_token::{decode_unverified_payload, verify_signature};

    use super::*;

    #[test]
    fn the_peer_is_a_broadcast_of_the_room_other_than_a_call() {
        // Act / Assert
        assert_eq!(
            peer_of_room("app/room", "app/room/alice/call").unwrap(),
            vec!["app", "room", "alice", "call"]
        );
        assert!(matches!(
            peer_of_room("app/room", "app/room2/alice"),
            Err(PlaceError::InvalidPeer)
        ));
        assert!(matches!(
            peer_of_room("app/room", "app/room"),
            Err(PlaceError::InvalidPeer)
        ));
        assert!(matches!(
            peer_of_room("app/room", "app/room/phone/1234"),
            Err(PlaceError::InvalidPeer)
        ));
        assert!(matches!(
            peer_of_room("app/room", "app/room/alice//call"),
            Err(PlaceError::InvalidPeer)
        ));
    }

    #[test]
    fn the_relay_token_publishes_the_call_and_subscribes_to_the_peer_only() {
        // Act
        let token = relay_token(
            "app",
            "app-secret",
            "app/room/phone/abc",
            "app/room/alice/call",
            100,
            Duration::from_secs(3_900),
        )
        .unwrap();

        // Assert
        verify_signature(&token, "app-secret").unwrap();
        let payload = decode_unverified_payload(&token).unwrap();
        assert_eq!(payload["publish"], "room/phone/abc");
        assert_eq!(payload["subscribe"], "room/alice/call");
        assert_eq!(payload["exp"], 4_000);
    }
}
