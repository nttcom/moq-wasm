use anyhow::Ok;
use async_trait::async_trait;
use std::{net::SocketAddr, sync::Arc};

use quinn::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use quinn::{self, TransportConfig, VarInt};

use crate::modules::transport::transport_connection::TransportConnection;
use crate::modules::transport::{
    quic::quic_connection::QUICConnection, transport_connection_creator::TransportConnectionCreator,
};

pub(crate) struct QUICConnectionCreator {
    endpoint: quinn::Endpoint,
}

impl QUICConnectionCreator {
    fn config_builder(
        cert_path: String,
        key_path: String,
        port_num: u16,
        keep_alive_sec: u64,
    ) -> anyhow::Result<quinn::ServerConfig> {
        let cert = vec![CertificateDer::from_pem_file(cert_path).inspect_err(|e| {
            tracing::error!("Creating certificate failed: {:?}", e.to_string())
        })?];
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

        let transport_arc = Arc::new(transport_config);
        server_config.transport_config(transport_arc);

        Ok(server_config)
    }

    pub fn client(port_num: u16) -> anyhow::Result<Self> {
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().unwrap() {
            roots.add(cert).unwrap();
        }

        Self::create_client(port_num, roots)
    }

    pub fn client_with_custom_cert(port_num: u16, custom_cert_path: &str) -> anyhow::Result<Self> {
        let cert = CertificateDer::from_pem_file(custom_cert_path).inspect_err(|e| {
            tracing::error!("Creating certificate failed: {:?}", e.to_string())
        })?;

        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert).unwrap();

        Self::create_client(port_num, roots)
    }

    fn create_client(port_num: u16, root_cert: rustls::RootCertStore) -> anyhow::Result<Self> {
        let address = SocketAddr::from(([0, 0, 0, 0], port_num));
        let mut endpoint = quinn::Endpoint::client(address)?;

        let mut client_crypto = rustls::ClientConfig::builder()
            .with_root_certificates(root_cert)
            .with_no_client_auth();

        let alpn = &[b"moq-00"];
        client_crypto.alpn_protocols = alpn.iter().map(|&x| x.into()).collect();
        client_crypto.key_log = Arc::new(rustls::KeyLogFile::new());

        let client_config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)?,
        ));
        endpoint.set_default_client_config(client_config);

        tracing::info!("Client ready! for QUIC");
        Ok(QUICConnectionCreator { endpoint })
    }

    pub fn server(
        cert_path: String,
        key_path: String,
        port_num: u16,
        keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        let server_config = Self::config_builder(cert_path, key_path, port_num, keep_alive_sec)?;
        let address = SocketAddr::from(([0, 0, 0, 0], port_num));
        let endpoint = quinn::Endpoint::server(server_config, address)?;
        tracing::info!("Server ready! for QUIC");
        Ok(QUICConnectionCreator { endpoint })
    }
}

#[async_trait]
impl TransportConnectionCreator for QUICConnectionCreator {
    async fn create_new_transport(
        &self,
        remote_address: SocketAddr,
        host: &str
    ) -> anyhow::Result<Box<dyn TransportConnection>> {
        let connecting = self
            .endpoint
            .connect(remote_address, host)
            .inspect_err(|e| tracing::error!("failed to connect: {:?}", e.to_string()))?;
        let connection = connecting
            .await
            .inspect_err(|e| tracing::error!("failed to create connection: {:?}", e.to_string()))?;

        Ok(Box::new(QUICConnection::new(connection)))
    }

    async fn accept_new_transport(&mut self) -> anyhow::Result<Box<dyn TransportConnection>> {
        let incoming = self.endpoint.accept().await.expect("failed to accept");
        let connection = incoming.await.expect("failed to create connection");

        Ok(Box::new(QUICConnection::new(connection)))
    }
}
