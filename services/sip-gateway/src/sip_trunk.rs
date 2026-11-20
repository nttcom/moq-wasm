use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result, anyhow};
use rsipstack::{
    EndpointBuilder,
    dialog::{
        DialogId,
        authenticate::Credential,
        dialog::{DialogState, DialogStateReceiver},
        dialog_layer::DialogLayer,
        invitation::{InviteAsyncResult, InviteOption},
        invite_dialog::InviteDialog,
    },
    sip::{StatusCode, Uri},
    transport::{
        TransportLayer,
        tls::{TlsConfig, TlsListenerConnection},
    },
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const USER_AGENT: &str = "moq-wasm sip-gateway";

/// How the gateway reaches the trunk.
pub(crate) struct TrunkSettings {
    /// `host[:port]` of the trunk's TLS signalling (5061 when absent), also
    /// the host of every Request-URI and the name its certificate must carry.
    pub(crate) host: String,
    /// PEM roots the trunk's certificate is verified against.
    pub(crate) ca_certificates: Vec<u8>,
    /// Answers a 401/407 to an INVITE (digest, RFC 3261 §22).
    pub(crate) credential: Option<(String, String)>,
}

/// Where the gateway takes SIP over TLS. Its address is also the Via and
/// Contact of every request, so the trunk's in-dialog requests reach it.
pub(crate) struct ListenSettings {
    pub(crate) address: SocketAddr,
    /// The address the trunk reaches `address` at, when it is behind NAT.
    pub(crate) public_address: Option<SocketAddr>,
    /// PEM certificate chain and private key the listener presents.
    pub(crate) certificate: Vec<u8>,
    pub(crate) private_key: Vec<u8>,
}

/// The gateway's SIP user agent towards one trunk over TLS. It sends no
/// REGISTER: the trunk knows the gateway by its address and credentials.
pub(crate) struct SipTrunk {
    dialog_layer: Arc<DialogLayer>,
    host: String,
    credential: Option<Credential>,
    cancel: CancellationToken,
    _endpoint_task: JoinHandle<()>,
    _request_task: JoinHandle<()>,
}

/// An INVITE the trunk has not finished answering, then the dialog it set up.
pub(crate) struct OutboundInvite {
    pub(crate) dialog: InviteDialog,
    pub(crate) states: DialogStateReceiver,
    pub(crate) result: rsipstack::platform::JoinHandle<InviteAsyncResult>,
}

impl SipTrunk {
    /// Fails when the TLS listener cannot bind or load its certificate and key.
    pub(crate) async fn run(listen: ListenSettings, settings: TrunkSettings) -> Result<Self> {
        let cancel = CancellationToken::new();
        let transport_layer = TransportLayer::new(cancel.child_token());
        let tls_config = TlsConfig {
            cert: Some(listen.certificate),
            key: Some(listen.private_key),
            ca_certs: Some(settings.ca_certificates),
            ..TlsConfig::default()
        };
        let listener =
            TlsListenerConnection::new(listen.address, listen.public_address, tls_config.clone())
                .await
                .context("open the SIP TLS listener")?;
        transport_layer.add_transport(listener.into());
        transport_layer.set_tls_config(tls_config);
        let endpoint = EndpointBuilder::new()
            .with_user_agent(USER_AGENT)
            .with_transport_layer(transport_layer)
            .with_cancel_token(cancel.child_token())
            .build();
        let mut incoming = endpoint
            .incoming_transactions()
            .expect("a new endpoint hands out its transactions once");
        let dialog_layer = Arc::new(DialogLayer::new(endpoint.inner.clone()));
        let router = dialog_layer.clone();
        // In-dialog requests (BYE, re-INVITE) reach their dialog; any other
        // request is refused.
        let request_task = tokio::spawn(async move {
            while let Some(mut transaction) = incoming.recv().await {
                let result = match router.match_dialog(&transaction) {
                    Some(mut dialog) => dialog.handle(&mut transaction).await,
                    None => {
                        transaction
                            .reply(StatusCode::CallTransactionDoesNotExist)
                            .await
                    }
                };
                if let Err(error) = result {
                    tracing::warn!(%error, "SIP request not handled");
                }
            }
        });
        let endpoint_task = tokio::spawn(async move { endpoint.serve().await });
        Ok(Self {
            dialog_layer,
            host: settings.host,
            credential: settings.credential.map(|(username, password)| Credential {
                username,
                password,
                realm: None,
                auth_username: None,
            }),
            cancel,
            _endpoint_task: endpoint_task,
            _request_task: request_task,
        })
    }

    /// Sends an INVITE for `to` from `from` (E.164 numbers, or a user name
    /// for `from`) carrying `offer`, with `call_id` as its Call-ID.
    pub(crate) fn invite(
        &self,
        call_id: &str,
        from: &str,
        to: &str,
        offer: String,
    ) -> Result<OutboundInvite> {
        let option = InviteOption {
            call_id: Some(call_id.to_string()),
            caller: self.uri(from)?,
            callee: self.uri(to)?,
            contact: Uri::try_from("sip:sip-gateway@0.0.0.0")
                .map_err(|error| anyhow!("{error}"))?,
            content_type: Some("application/sdp".to_string()),
            offer: Some(offer.into_bytes()),
            credential: self.credential.clone(),
            ..InviteOption::default()
        };
        let (states_sender, states) = self.dialog_layer.new_dialog_state_channel();
        let (dialog, result) = self
            .dialog_layer
            .do_invite_async(option, states_sender)
            .context("send INVITE")?;
        Ok(OutboundInvite {
            dialog,
            states,
            result,
        })
    }

    /// Forgets a dialog that has ended; the dialog layer keeps a confirmed
    /// one until it is removed.
    pub(crate) fn remove_dialog(&self, id: &DialogId) {
        self.dialog_layer.remove_dialog(id);
    }

    fn uri(&self, user: &str) -> Result<Uri> {
        Uri::try_from(format!("sip:{user}@{};transport=tls", self.host).as_str())
            .map_err(|error| anyhow!("SIP URI for {user}: {error}"))
    }
}

impl Drop for SipTrunk {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What a dialog state means to the call.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    /// 180 or 183, with its status.
    Ringing(u16),
    /// The 2xx and its SDP answer.
    Answered(String),
    Ended,
    Other,
}

pub(crate) fn progress(state: &DialogState) -> Progress {
    match state {
        DialogState::Early(_, response) if matches!(response.status_code.code(), 180 | 183) => {
            Progress::Ringing(response.status_code.code())
        }
        DialogState::Confirmed(_, response) => {
            Progress::Answered(String::from_utf8_lossy(&response.body).into_owned())
        }
        DialogState::Terminated(..) => Progress::Ended,
        _ => Progress::Other,
    }
}
