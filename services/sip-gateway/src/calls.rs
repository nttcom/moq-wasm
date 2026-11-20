use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use auth_token::{Claims, sign_token, unix_now};
use serde::Deserialize;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    call::{self, CallContext, CallPlan},
    call_auth::AuthorizedCall,
};

/// The caller an INVITE shows for an app without a number of its own.
const ANONYMOUS_CALLER: &str = "anonymous";
/// The relay token outlives the longest call by this much.
const RELAY_TOKEN_MARGIN: Duration = Duration::from_secs(5 * 60);

/// The calls in progress, at most `max_calls_per_app` for each app.
pub(crate) struct Calls {
    context: Arc<CallContext>,
    max_calls_per_app: usize,
    active: Arc<Mutex<HashMap<String, ActiveCall>>>,
    tasks: TaskTracker,
}

struct ActiveCall {
    app_id: String,
    hang_up: CancellationToken,
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
    pub(crate) fn new(context: CallContext, max_calls_per_app: usize) -> Self {
        Self {
            context: Arc::new(context),
            max_calls_per_app,
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
        let peer = peer_of_room(&request.namespace_prefix, &request.peer)?;
        let call_id = random_hex();
        let namespace = format!("{}/phone/{call_id}", request.namespace_prefix);
        let relay_token = match &authorized.secret {
            Some(secret) => Some(
                relay_token(
                    &authorized.app_id,
                    secret,
                    &namespace,
                    &request.peer,
                    unix_now(),
                    self.context.max_duration + RELAY_TOKEN_MARGIN,
                )
                .map_err(PlaceError::Failed)?,
            ),
            None => None,
        };
        let hang_up = CancellationToken::new();
        {
            let mut active = self.active.lock().expect("calls lock");
            let in_progress = active
                .values()
                .filter(|call| call.app_id == authorized.app_id)
                .count();
            if in_progress >= self.max_calls_per_app {
                return Err(PlaceError::Busy);
            }
            active.insert(
                call_id.clone(),
                ActiveCall {
                    app_id: authorized.app_id.clone(),
                    hang_up: hang_up.clone(),
                },
            );
        }
        let plan = CallPlan {
            call_id: call_id.clone(),
            app_id: authorized.app_id,
            from: authorized
                .from
                .unwrap_or_else(|| ANONYMOUS_CALLER.to_string()),
            to: request.to,
            namespace: split(&namespace),
            peer,
            relay_token,
        };
        let context = self.context.clone();
        let active = self.active.clone();
        let id = call_id.clone();
        self.tasks.spawn(async move {
            call::run(context, plan, hang_up).await;
            active.lock().expect("calls lock").remove(&id);
        });
        Ok(PlacedCall { call_id, namespace })
    }

    /// Ends every call with BYE (or CANCEL) and waits until they have ended.
    pub(crate) async fn hang_up_all(&self) {
        for call in self.active.lock().expect("calls lock").values() {
            call.hang_up.cancel();
        }
        self.tasks.close();
        self.tasks.wait().await;
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
