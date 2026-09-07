use anyhow::Ok;
use async_trait::async_trait;
use std::{
    net::{Ipv6Addr, SocketAddr},
    sync::Arc,
};

use quinn::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use quinn::{self, TransportConfig, VarInt};

use crate::modules::transport::{
    client_crypto::{
        MOQ_ALPN, client_crypto, client_crypto_with_custom_cert, client_endpoint,
        quic_client_config,
    },
    connect_target::{ClientTransport, ConnectTarget},
    crypto_provider::install_default_crypto_provider,
    quic::quic_connection::QUICConnection,
    transport_connection_creator::TransportConnectionCreator,
};

pub struct QUICConnectionCreator {
    endpoint: quinn::Endpoint,
}

impl QUICConnectionCreator {
    fn config_builder(
        cert_path: &str,
        key_path: &str,
        keep_alive_sec: u64,
    ) -> anyhow::Result<quinn::ServerConfig> {
        install_default_crypto_provider();

        let cert = CertificateDer::pem_file_iter(cert_path)
            .inspect_err(|e| tracing::error!("Opening certificate file failed: {:?}", e))?
            .collect::<Result<Vec<_>, _>>()
            .inspect_err(|e| tracing::error!("Parsing certificates failed: {:?}", e))?;
        let key = PrivateKeyDer::from_pem_file(key_path)
            .inspect_err(|e| tracing::error!("Creating private key failed: {:?}", e.to_string()))?;
        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert, key)
            .inspect_err(|e| tracing::error!("server config failed: {:?}", e.to_string()))?;
        let alpn = &[b"moq-00"];
        server_crypto.alpn_protocols = alpn.iter().map(|&x| x.into()).collect();
        server_crypto.key_log = Arc::new(rustls::KeyLogFile::new());

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
        ));
        let mut transport_config = TransportConfig::default();
        let keep_alive_sec = std::time::Duration::from_secs(keep_alive_sec);
        transport_config.keep_alive_interval(Some(keep_alive_sec));
        // 単方向ストリーム数を100000に設定
        transport_config.max_concurrent_uni_streams(100000u32.into());
        // initial_max_stream_data_uniと同義。デフォルトは65,536 バイト (64KB) 大きくするとACKを待たずに送信するため、輻輳が発生する可能性が高まる
        transport_config.send_window(64 * 1024);
        // パケロス判定して再送を要求するまでの時間(RTTの倍数)を指定する。小さくすると再送が増える Default(RFC推奨値): 1.125
        // transport_config.time_threshold(1.5);
        // パケロス判定して再送を要求するまでのパケット間隔を指定する。小さくすると再送が増える Default(RFC推奨値): 3
        transport_config.packet_threshold(5);
        transport_config.stream_receive_window(VarInt::from_u32(1024 * 1024)); // initial_max_stream_data_uniと同義。デフォルトは65,536 バイト (64KB)なので1MBにする

        tracing::warn!("datagram setting: {:?}", transport_config);

        let transport_arc = Arc::new(transport_config);
        server_config.transport_config(transport_arc);

        Ok(server_config)
    }

    fn create_client(port_num: u16, crypto: rustls::ClientConfig) -> anyhow::Result<Self> {
        let mut endpoint = client_endpoint(port_num)?;
        endpoint.set_default_client_config(quic_client_config(crypto, MOQ_ALPN)?);
        tracing::info!("Client ready! for QUIC: {:?}", endpoint.local_addr()?);
        Ok(QUICConnectionCreator { endpoint })
    }
}

#[async_trait]
impl TransportConnectionCreator for QUICConnectionCreator {
    type Connection = QUICConnection;

    fn client(port_num: u16, verify_certificate: bool) -> anyhow::Result<Self> {
        Self::create_client(port_num, client_crypto(verify_certificate)?)
    }

    fn client_with_custom_cert(port_num: u16, custom_cert_path: &str) -> anyhow::Result<Self> {
        Self::create_client(port_num, client_crypto_with_custom_cert(custom_cert_path)?)
    }

    fn server(
        cert_path: &str,
        key_path: &str,
        port_num: u16,
        keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        let server_config = Self::config_builder(cert_path, key_path, keep_alive_sec)?;
        let address = SocketAddr::from((Ipv6Addr::UNSPECIFIED, port_num));
        let endpoint = quinn::Endpoint::server(server_config, address)?;
        tracing::info!("Server ready! for QUIC: {:?}", address);
        Ok(QUICConnectionCreator { endpoint })
    }

    async fn create_new_transport(
        &self,
        target: &ConnectTarget,
    ) -> anyhow::Result<Self::Connection> {
        if target.transport != ClientTransport::Quic {
            anyhow::bail!("QUIC endpoint requires a moqt:// url, got {}", target.url);
        }
        let remote_address = target.resolve_remote_address().await?;
        let connecting = self
            .endpoint
            .connect(remote_address, &target.server_name())
            .inspect_err(|e| tracing::error!("failed to connect: {:?}", e.to_string()))?;
        let connection = connecting
            .await
            .inspect_err(|e| tracing::error!("failed to create connection: {:?}", e.to_string()))?;

        Ok(QUICConnection::new(connection))
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

        Ok(QUICConnection::new(connection))
    }
}
