mod call;
mod call_auth;
mod calls;
mod media;
mod server;
mod sip_trunk;
mod webhook;

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use app_registry::{Location, LocationVariables, RegistryStore};
use telemetry_otlp::non_empty_env;
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};

use crate::{
    call::CallContext,
    call_auth::CallAuth,
    calls::{Calls, InboundTask},
    server::GatewayServer,
    sip_trunk::{ListenSettings, SipTrunk, TrunkSettings, parse_ranges},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = telemetry_otlp::init(telemetry_otlp::TelemetryConfig {
        service_name: "sip-gateway",
        service_version: env!("CARGO_PKG_VERSION"),
        // rsipstack logs every SIP message whole at INFO, SRTP keys and
        // digest responses included.
        default_directives: &["sip_gateway=info", "broadcast=info", "rsipstack=warn"],
        log_output: telemetry_otlp::LogOutput::Stdout,
        ..Default::default()
    })?;
    // rsipstack's TLS and reqwest both use rustls; the process picks its
    // provider once.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let required = |name: &str| non_empty_env(name).with_context(|| format!("{name} must be set"));
    let number = |name: &str, default: u64| -> anyhow::Result<u64> {
        non_empty_env(name).map_or(Ok(default), |value| {
            value
                .parse()
                .with_context(|| format!("{name} must be a number"))
        })
    };
    let port = u16::try_from(number("PORT", 8099)?).context("PORT must be a port number")?;
    let credential = non_empty_env("SIP_GATEWAY_TRUNK_USERNAME")
        .map(|username| required("SIP_GATEWAY_TRUNK_PASSWORD").map(|password| (username, password)))
        .transpose()?;
    let read = |name: &str| -> anyhow::Result<Vec<u8>> {
        let path = required(name)?;
        std::fs::read(&path).with_context(|| format!("read {path}"))
    };
    let listen = ListenSettings {
        address: non_empty_env("SIP_GATEWAY_SIP_ADDRESS")
            .unwrap_or_else(|| "0.0.0.0:5061".to_string())
            .parse()
            .context("SIP_GATEWAY_SIP_ADDRESS must be a socket address")?,
        public_address: non_empty_env("SIP_GATEWAY_SIP_PUBLIC_ADDRESS")
            .map(|address| address.parse())
            .transpose()
            .context("SIP_GATEWAY_SIP_PUBLIC_ADDRESS must be a socket address")?,
        certificate: read("SIP_GATEWAY_TLS_CERT_FILE")?,
        private_key: read("SIP_GATEWAY_TLS_KEY_FILE")?,
    };
    let signalling_addresses =
        parse_ranges(&non_empty_env("SIP_GATEWAY_TRUNK_SIGNALLING_ADDRESSES").unwrap_or_default())
            .context(
                "SIP_GATEWAY_TRUNK_SIGNALLING_ADDRESSES must list IP addresses or CIDR blocks",
            )?;
    let (trunk, invites) = SipTrunk::run(
        listen,
        TrunkSettings {
            host: required("SIP_GATEWAY_TRUNK_HOST")?,
            ca_certificates: read("SIP_GATEWAY_TRUNK_CA_FILE")?,
            credential,
            signalling_addresses,
        },
    )
    .await?;
    let media_address = non_empty_env("SIP_GATEWAY_MEDIA_ADDRESS")
        .map(|address| address.parse())
        .transpose()
        .context("SIP_GATEWAY_MEDIA_ADDRESS must be an IP address")?;
    let context = CallContext {
        trunk,
        relay_url: required("SIP_GATEWAY_RELAY_URL")?,
        verify_relay_certificate: non_empty_env("SIP_GATEWAY_RELAY_INSECURE_SKIP_TLS_VERIFY")
            .is_none_or(|value| value != "true"),
        media_address,
        max_duration: Duration::from_secs(number("SIP_GATEWAY_MAX_CALL_SECONDS", 3_600)?),
    };
    let max_calls_per_app = usize::try_from(number("SIP_GATEWAY_MAX_CALLS_PER_APP", 2)?)?;

    let (call_auth, registry) = if non_empty_env("SIP_GATEWAY_INSECURE_SKIP_AUTH")
        .is_some_and(|v| v == "true")
    {
        tracing::warn!(
            "SIP_GATEWAY_INSECURE_SKIP_AUTH: every request and destination is accepted, calls join with no token and INVITEs from the trunk are refused"
        );
        (CallAuth::Insecure, None)
    } else {
        let registry = Location::from_env(&LocationVariables {
            file: "SIP_GATEWAY_REGISTRY_FILE",
            bucket: "SIP_GATEWAY_REGISTRY_BUCKET",
            object: "SIP_GATEWAY_REGISTRY_OBJECT",
        })?
        .context("set SIP_GATEWAY_REGISTRY_FILE or SIP_GATEWAY_REGISTRY_BUCKET")?;
        let registry = Arc::new(RegistryStore::new(registry)?);
        (
            CallAuth::new(required("SIP_GATEWAY_VTS_URL")?, registry.clone()),
            Some(registry),
        )
    };
    let calls = Arc::new(Calls::new(context, max_calls_per_app, registry));
    let _inbound = InboundTask::run(calls.clone(), invites);

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let _server = GatewayServer::run(listener, calls.clone(), call_auth);
    tracing::info!(port, "sip-gateway listening");

    // As PID 1 in a container the process has no default SIGTERM action.
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = terminate.recv() => {}
    }
    tracing::info!("sip-gateway shutting down");
    calls.hang_up_all().await;
    Ok(())
}
