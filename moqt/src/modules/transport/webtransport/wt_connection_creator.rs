use std::net::SocketAddr;

use async_trait::async_trait;

use super::wt_connection::WtConnection;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;

enum WtEndpoint {
    Server(wtransport::Endpoint<wtransport::endpoint::endpoint_side::Server>),
    Client(wtransport::Endpoint<wtransport::endpoint::endpoint_side::Client>),
}

pub struct WtConnectionCreator {
    endpoint: WtEndpoint,
}

#[async_trait]
impl TransportConnectionCreator for WtConnectionCreator {
    type Connection = WtConnection;

    fn client(port_num: u16, verify_certificate: bool) -> anyhow::Result<Self> {
        use wtransport::tls::rustls;

        let tls_config = if verify_certificate {
            let mut roots = rustls::RootCertStore::empty();
            for cert in rustls_native_certs::load_native_certs().unwrap() {
                roots.add(cert).unwrap();
            }
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth()
        } else {
            // Currently quinn and wtransport share the same rustls version,
            // so we reuse the QUIC SkipVerification to avoid duplication.
            // If their rustls versions diverge, this may need a separate implementation.
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(std::sync::Arc::new(
                    crate::modules::transport::quic::skip_certd_validation::SkipVerification,
                ))
                .with_no_client_auth()
        };
        let mut tls_config = tls_config;
        tls_config.alpn_protocols = vec![wtransport::tls::WEBTRANSPORT_ALPN.to_vec()];

        let config = wtransport::ClientConfig::builder()
            .with_bind_default()
            .with_custom_tls(tls_config)
            .build();

        let endpoint = wtransport::Endpoint::client(config)?;
        tracing::info!("Client ready! for WebTransport port: {}", port_num);

        Ok(WtConnectionCreator {
            endpoint: WtEndpoint::Client(endpoint),
        })
    }

    fn client_with_custom_cert(port_num: u16, custom_cert_path: &str) -> anyhow::Result<Self> {
        use std::fs::File;
        use std::io::BufReader;
        use wtransport::tls::rustls;

        // 証明書を読み込む
        let cert_file = File::open(custom_cert_path)
            .inspect_err(|e| tracing::error!("Opening certificate file failed: {:?}", e))?;
        let mut cert_reader = BufReader::new(cert_file);
        let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .inspect_err(|e| tracing::error!("Parsing certificate failed: {:?}", e))?;

        // 自己署名証明書を信頼するrustls ClientConfigを作る
        let mut roots = rustls::RootCertStore::empty();
        for cert in certs {
            roots
                .add(cert)
                .inspect_err(|e| tracing::error!("Adding certificate failed: {:?}", e))?;
        }
        let mut tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        tls_config.alpn_protocols = vec![wtransport::tls::WEBTRANSPORT_ALPN.to_vec()];

        // wtransportのクライアント設定を組み立てる
        let config = wtransport::ClientConfig::builder()
            .with_bind_default()
            .with_custom_tls(tls_config)
            .build();

        // クライアントEndpointを作る
        let endpoint = wtransport::Endpoint::client(config)?;
        tracing::info!("Client ready! for WebTransport port: {}", port_num);

        Ok(WtConnectionCreator {
            endpoint: WtEndpoint::Client(endpoint),
        })
    }

    fn server(
        cert_path: &str,
        key_path: &str,
        port_num: u16,
        keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        use std::fs::File;
        use std::io::BufReader;

        // 証明書を同期的に読み込む
        let cert_file = File::open(cert_path)
            .inspect_err(|e| tracing::error!("Opening certificate file failed: {:?}", e))?;
        let mut cert_reader = BufReader::new(cert_file);
        let certs: Vec<wtransport::tls::Certificate> = rustls_pemfile::certs(&mut cert_reader)
            .filter_map(|c| c.ok())
            .map(|der| wtransport::tls::Certificate::from_der(der.as_ref().to_vec()))
            .collect::<Result<Vec<_>, _>>()
            .inspect_err(|e| tracing::error!("Parsing certificates failed: {:?}", e))?;

        // 秘密鍵を同期的に読み込む
        let key_file = File::open(key_path)
            .inspect_err(|e| tracing::error!("Opening key file failed: {:?}", e))?;
        let mut key_reader = BufReader::new(key_file);
        let key_der = rustls_pemfile::private_key(&mut key_reader)?
            .ok_or_else(|| anyhow::anyhow!("No private key found"))?;
        let private_key =
            wtransport::tls::PrivateKey::from_der_pkcs8(key_der.secret_der().to_vec());

        let identity =
            wtransport::Identity::new(wtransport::tls::CertificateChain::new(certs), private_key);

        let config = wtransport::ServerConfig::builder()
            .with_bind_default(port_num)
            .with_identity(identity)
            .keep_alive_interval(Some(std::time::Duration::from_secs(keep_alive_sec)))
            .build();

        let endpoint = wtransport::Endpoint::server(config)?;
        tracing::info!("Server ready! for WebTransport port: {}", port_num);

        Ok(WtConnectionCreator {
            endpoint: WtEndpoint::Server(endpoint),
        })
    }

    async fn create_new_transport(
        &self,
        remote_address: SocketAddr,
        host: &str,
    ) -> anyhow::Result<Self::Connection> {
        let ep = match &self.endpoint {
            WtEndpoint::Client(ep) => ep,
            WtEndpoint::Server(_) => {
                anyhow::bail!("Cannot create_new_transport on a server endpoint")
            }
        };
        let url = format!("https://{}:{}", host, remote_address.port());

        let connection = ep
            .connect(url)
            .await
            .inspect_err(|e| tracing::error!("failed to connect: {:?}", e))?;

        Ok(WtConnection::new(connection))
    }

    async fn accept_new_transport(&mut self) -> anyhow::Result<Self::Connection> {
        let ep = match &self.endpoint {
            WtEndpoint::Server(ep) => ep,
            WtEndpoint::Client(_) => {
                anyhow::bail!("Cannot accept_new_transport on a client endpoint")
            }
        };
        // クライアントの接続を待つ
        let incoming_session = ep.accept().await;

        // セッションリクエストを待つ
        let session_request = incoming_session
            .await
            .inspect_err(|e| tracing::error!("failed to accept session: {:?}", e))?;

        // リクエストを受け入れてセッションを確立する
        let connection = session_request
            .accept()
            .await
            .inspect_err(|e| tracing::error!("failed to establish connection: {:?}", e))?;

        Ok(WtConnection::new(connection))
    }
}
