use std::{sync::Arc, time::Duration};

use anyhow::Context;
use quinn::{
    TransportConfig, VarInt,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};

use crate::modules::transport::crypto_provider::install_default_crypto_provider;

/// draft-18 keeps a bidirectional stream open for every request in flight,
/// so the peer may open this many before one ends.
pub(crate) const MAX_CONCURRENT_BIDI_STREAMS: u32 = 1000;

pub(crate) fn server_transport_config(keep_alive_sec: u64) -> TransportConfig {
    let mut transport_config = TransportConfig::default();
    transport_config.keep_alive_interval(Some(Duration::from_secs(keep_alive_sec)));
    // quinn allocates state for every remote stream ID up to this limit on each connection.
    transport_config.max_concurrent_uni_streams(1000u32.into());
    transport_config.max_concurrent_bidi_streams(MAX_CONCURRENT_BIDI_STREAMS.into());
    transport_config.packet_threshold(5);
    transport_config.stream_receive_window(VarInt::from_u32(1024 * 1024));
    transport_config
}

fn load_cert_and_key(
    cert_path: &str,
    key_path: &str,
) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    install_default_crypto_provider();
    let cert = CertificateDer::pem_file_iter(cert_path)
        .context("failed to open the certificate file")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to parse the certificate file")?;
    let key =
        PrivateKeyDer::from_pem_file(key_path).context("failed to read the private key file")?;
    Ok((cert, key))
}

pub(crate) fn quic_server_config(
    cert_path: &str,
    key_path: &str,
    keep_alive_sec: u64,
    alpn_protocols: Vec<Vec<u8>>,
) -> anyhow::Result<quinn::ServerConfig> {
    let (cert, key) = load_cert_and_key(cert_path, key_path)?;
    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert, key)
        .context("failed to build the TLS server config")?;
    server_crypto.alpn_protocols = alpn_protocols;
    server_crypto.key_log = Arc::new(rustls::KeyLogFile::new());

    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
    ));
    server_config.transport_config(Arc::new(server_transport_config(keep_alive_sec)));
    Ok(server_config)
}
