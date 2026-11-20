mod address_range;

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
    sip::{Header, Method, StatusCode, Uri, prelude::HeadersExt},
    transaction::transaction::Transaction,
    transport::{
        TransportLayer,
        tls::{TlsConfig, TlsListenerConnection},
    },
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub(crate) use address_range::{AddressRange, parse_ranges};

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
    /// The addresses the trunk sends its INVITEs from; an INVITE from any
    /// other is refused with 403.
    pub(crate) signalling_addresses: Vec<AddressRange>,
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

/// An INVITE from the trunk, not answered yet.
/// The gateway answers it with `ringing`, `accept` or `reject` on `dialog`.
pub(crate) struct InboundInvite {
    pub(crate) dialog: InviteDialog,
    pub(crate) states: DialogStateReceiver,
    /// The caller's number as the trunk shows it, or `anonymous`.
    pub(crate) from: String,
    /// The number dialled: the user of the Request-URI.
    pub(crate) to: String,
    pub(crate) offer: String,
}

/// An INVITE the trunk has not finished answering, then the dialog it set up.
pub(crate) struct OutboundInvite {
    pub(crate) dialog: InviteDialog,
    pub(crate) states: DialogStateReceiver,
    pub(crate) result: rsipstack::platform::JoinHandle<InviteAsyncResult>,
}

impl SipTrunk {
    /// Also returns the INVITEs the trunk sends to start a call. Fails when
    /// the TLS listener cannot bind or load its certificate and key.
    pub(crate) async fn run(
        listen: ListenSettings,
        settings: TrunkSettings,
    ) -> Result<(Self, mpsc::UnboundedReceiver<InboundInvite>)> {
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
        let signalling_addresses = settings.signalling_addresses;
        let (invite_sender, invite_receiver) = mpsc::unbounded_channel();
        // In-dialog requests (BYE, re-INVITE) reach the dialog they belong
        // to; an INVITE outside a dialog starts an inbound call.
        let request_task = tokio::spawn(async move {
            while let Some(mut transaction) = incoming.recv().await {
                let result = match router.match_dialog(&transaction) {
                    Some(mut dialog) => dialog.handle(&mut transaction).await,
                    None if transaction.original.method == Method::Invite => {
                        receive_invite(&router, &signalling_addresses, transaction, &invite_sender)
                            .await
                            .map_err(|error| *error)
                    }
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
        let trunk = Self {
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
        };
        Ok((trunk, invite_receiver))
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

/// Refuses an INVITE from outside `signalling_addresses` with 403; otherwise
/// hands it over as an `InboundInvite`, with a task driving its transaction
/// until the ACK or CANCEL.
async fn receive_invite(
    router: &DialogLayer,
    signalling_addresses: &[AddressRange],
    mut transaction: Transaction,
    invite_sender: &mpsc::UnboundedSender<InboundInvite>,
) -> Result<(), Box<rsipstack::Error>> {
    let source = transaction
        .connection
        .as_ref()
        .and_then(|connection| connection.get_remote_addr())
        .and_then(|address| address.get_socketaddr().ok())
        .map(|address| address.ip());
    let from_trunk = source.is_some_and(|source| {
        signalling_addresses
            .iter()
            .any(|range| range.contains(source))
    });
    if !from_trunk {
        tracing::warn!(remote_address = ?source, "INVITE from outside the trunk refused");
        return Ok(transaction.reply(StatusCode::Forbidden).await?);
    }
    let request = &transaction.original;
    let sip_call_id = request
        .call_id_header()
        .map(|call_id| call_id.value().to_string())
        .unwrap_or_default();
    let Some(to) = request.uri.user().map(str::to_string) else {
        return Ok(transaction.reply(StatusCode::NotFound).await?);
    };
    let from = request
        .from_header()
        .ok()
        .and_then(|from| from.uri().ok())
        .and_then(|uri| uri.user().map(str::to_string))
        .unwrap_or_else(|| "anonymous".to_string());
    let offer = String::from_utf8_lossy(&request.body).into_owned();
    tracing::info!(sip_call_id, remote_address = ?source, "INVITE received");
    let (states_sender, states) = router.new_dialog_state_channel();
    let dialog = router.get_or_create_server_invite(&transaction, states_sender, None, None)?;
    let mut handler = dialog.clone();
    tokio::spawn(async move {
        if let Err(error) = handler.handle(&mut transaction).await {
            tracing::warn!(%error, "INVITE transaction failed");
        }
    });
    let _ = invite_sender.send(InboundInvite {
        dialog,
        states,
        from,
        to,
        offer,
    });
    Ok(())
}

/// The header an answer with SDP carries.
pub(crate) fn sdp_content_type() -> Vec<Header> {
    vec![Header::ContentType("application/sdp".into())]
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
    /// The 2xx and its SDP answer; for an inbound call, the ACK to ours.
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
