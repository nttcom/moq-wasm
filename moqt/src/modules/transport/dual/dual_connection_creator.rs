use std::{
    fs::File,
    io::BufReader,
    net::{Ipv6Addr, SocketAddr},
    sync::Arc,
};

use async_trait::async_trait;
use quinn::rustls::{
    self,
    pki_types::{PrivateKeyDer, pem::PemObject},
};

use super::dual_connection::DualConnection;
use crate::modules::transport::{
    quic::quic_connection::QUICConnection,
    transport_connection_creator::TransportConnectionCreator,
    webtransport::wt_connection::WtConnection,
};

pub struct DualProtocolCreator {
    endpoint: quinn::Endpoint,
}

#[async_trait]
impl TransportConnectionCreator for DualProtocolCreator {
    type Connection = DualConnection;

    fn client(_port_num: u16, _verify_certificate: bool) -> anyhow::Result<Self> {
        anyhow::bail!("DualProtocolCreator does not support client mode")
    }

    fn client_with_custom_cert(_port_num: u16, _custom_cert_path: &str) -> anyhow::Result<Self> {
        anyhow::bail!("DualProtocolCreator does not support client mode")
    }

    fn server(
        cert_path: &str,
        key_path: &str,
        port_num: u16,
        keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        // 証明書を同期的に読み込む
        let cert = rustls_pemfile::certs(&mut BufReader::new(
            File::open(cert_path)
                .inspect_err(|e| tracing::error!("Opening certificate file failed: {:?}", e))?,
        ))
        .collect::<Result<Vec<_>, _>>()
        .inspect_err(|e| tracing::error!("Parsing certificates failed: {:?}", e))?;

        // 秘密鍵を同期的に読み込む
        let key = PrivateKeyDer::from_pem_file(key_path)
            .inspect_err(|e| tracing::error!("Creating private key failed: {:?}", e.to_string()))?;

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert, key)
            .inspect_err(|e| tracing::error!("server config failed: {:?}", e.to_string()))?;

        // ALPN を2つ登録（WebTransport + QUIC）
        server_crypto.alpn_protocols = vec![
            web_transport_quinn::ALPN.as_bytes().to_vec(), // h3
            b"moq-00".to_vec(),
        ];
        server_crypto.key_log = Arc::new(rustls::KeyLogFile::new());

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
        ));
        let mut transport_config = quinn::TransportConfig::default();
        let keep_alive_sec = std::time::Duration::from_secs(keep_alive_sec);
        transport_config.keep_alive_interval(Some(keep_alive_sec));
        transport_config.max_concurrent_uni_streams(100000u32.into());
        transport_config.send_window(64 * 1024);
        transport_config.packet_threshold(5);
        transport_config.stream_receive_window(quinn::VarInt::from_u32(1024 * 1024));

        let transport_arc = Arc::new(transport_config);
        server_config.transport_config(transport_arc);

        let address = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port_num));
        let endpoint = quinn::Endpoint::server(server_config, address)?;
        tracing::info!("Server ready! for Dual Protocol: {:?}", address);

        Ok(DualProtocolCreator { endpoint })
    }

    async fn create_new_transport(
        &self,
        _remote_address: SocketAddr,
        _host: &str,
    ) -> anyhow::Result<Self::Connection> {
        anyhow::bail!("DualProtocolCreator does not support client mode")
    }

    async fn accept_new_transport(&mut self) -> anyhow::Result<Self::Connection> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow::anyhow!("Endpoint is closing"))?;

        let connection = incoming
            .await
            .inspect_err(|e| tracing::error!("failed to create connection: {:?}", e.to_string()))?;

        // ALPN で分岐
        let alpn = connection
            .handshake_data()
            .and_then(|data| data.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
            .and_then(|data| data.protocol)
            .ok_or_else(|| anyhow::anyhow!("No ALPN protocol negotiated"))?;

        if alpn.as_slice() == web_transport_quinn::ALPN.as_bytes() {
            // WebTransport: H3 ハンドシェイクを行いセッションを確立する
            let request = web_transport_quinn::Request::accept(connection)
                .await
                .inspect_err(|e| {
                    tracing::error!("failed to accept WebTransport request: {:?}", e)
                })?;
            let session = request.ok().await.inspect_err(|e| {
                tracing::error!("failed to establish WebTransport session: {:?}", e)
            })?;
            Ok(DualConnection::WebTransport(Box::new(WtConnection::new(session))))
        } else if alpn.as_slice() == b"moq-00" {
            // Raw QUIC
            Ok(DualConnection::Quic(QUICConnection::new(connection)))
        } else {
            anyhow::bail!("Unsupported ALPN: {:?}", String::from_utf8_lossy(&alpn))
        }
    }
}
