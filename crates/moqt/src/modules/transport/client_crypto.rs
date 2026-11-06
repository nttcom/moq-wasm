use anyhow::Context;
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
};

use quinn::rustls::{
    self,
    pki_types::{CertificateDer, pem::PemObject},
};

use crate::modules::{
    moqt::domains::endpoint::CertificateVerification,
    transport::{
        crypto_provider::install_default_crypto_provider,
        quic::skip_certd_validation::SkipVerification,
    },
};

pub(crate) fn client_crypto(
    verification: &CertificateVerification,
) -> anyhow::Result<rustls::ClientConfig> {
    install_default_crypto_provider();
    let builder = rustls::ClientConfig::builder();
    let roots = match verification {
        CertificateVerification::NativeRoots => {
            let mut roots = rustls::RootCertStore::empty();
            for cert in rustls_native_certs::load_native_certs().certs {
                roots.add(cert)?;
            }
            roots
        }
        CertificateVerification::TrustedCertificates(path) => trusted_certificates(path)?,
        CertificateVerification::Disabled => {
            return Ok(builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(SkipVerification))
                .with_no_client_auth());
        }
    };
    Ok(builder.with_root_certificates(roots).with_no_client_auth())
}

fn trusted_certificates(path: &Path) -> anyhow::Result<rustls::RootCertStore> {
    let certs = CertificateDer::pem_file_iter(path)
        .with_context(|| format!("failed to open the certificate file {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to parse the certificate file {}", path.display()))?;
    anyhow::ensure!(
        !certs.is_empty(),
        "the certificate file {} holds no certificate",
        path.display()
    );
    let mut roots = rustls::RootCertStore::empty();
    for cert in certs {
        roots.add(cert)?;
    }
    Ok(roots)
}

pub(crate) fn quic_client_config(
    mut crypto: rustls::ClientConfig,
    alpn_protocols: Vec<Vec<u8>>,
) -> anyhow::Result<quinn::ClientConfig> {
    crypto.alpn_protocols = alpn_protocols;
    crypto.key_log = Arc::new(rustls::KeyLogFile::new());
    Ok(quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
    )))
}

pub(crate) fn client_endpoint(port_num: u16) -> anyhow::Result<quinn::Endpoint> {
    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port_num));
    Ok(quinn::Endpoint::client(address)?)
}
