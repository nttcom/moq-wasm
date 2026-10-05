use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};

use quinn::rustls::{
    self,
    pki_types::{CertificateDer, pem::PemObject},
};

use crate::modules::transport::{
    crypto_provider::install_default_crypto_provider, quic::skip_certd_validation::SkipVerification,
};

pub(crate) const MOQ_ALPN: &[u8] = b"moq-00";

pub(crate) fn client_crypto(verify_certificate: bool) -> anyhow::Result<rustls::ClientConfig> {
    install_default_crypto_provider();
    let builder = rustls::ClientConfig::builder();
    let crypto = if verify_certificate {
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().certs {
            roots.add(cert)?;
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipVerification))
            .with_no_client_auth()
    };
    Ok(crypto)
}

pub(crate) fn client_crypto_with_custom_cert(
    custom_cert_path: &str,
) -> anyhow::Result<rustls::ClientConfig> {
    install_default_crypto_provider();
    let certs = CertificateDer::pem_file_iter(custom_cert_path)
        .inspect_err(|e| tracing::error!("Opening certificate file failed: {:?}", e))?
        .collect::<Result<Vec<_>, _>>()
        .inspect_err(|e| tracing::error!("Parsing certificate failed: {:?}", e))?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in certs {
        roots.add(cert)?;
    }
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

pub(crate) fn quic_client_config(
    mut crypto: rustls::ClientConfig,
    alpn: &[u8],
) -> anyhow::Result<quinn::ClientConfig> {
    crypto.alpn_protocols = vec![alpn.to_vec()];
    crypto.key_log = Arc::new(rustls::KeyLogFile::new());
    Ok(quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
    )))
}

pub(crate) fn client_endpoint(port_num: u16) -> anyhow::Result<quinn::Endpoint> {
    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port_num));
    Ok(quinn::Endpoint::client(address)?)
}
