use anyhow::Ok;
use async_trait::async_trait;
use std::{net::SocketAddr, sync::Arc};

use quinn::{self, TransportConfig, VarInt};
use rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};

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
        let cert = vec![
            CertificateDer::from_pem_file(cert_path)
                .inspect(|e| tracing::error!("Creating certificate failed: {:?}", e))?,
        ];
        let key = PrivateKeyDer::from_pem_file(key_path)
            .inspect(|e| tracing::error!("Creating private key failed: {:?}", e))?;

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert, key)
            .inspect(|e| tracing::error!("{:?}", e))?;
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
        let address = SocketAddr::from(([0, 0, 0, 0], port_num));
        let endpoint = quinn::Endpoint::client(address)?;
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
        server_name: &str,
        port: u16,
    ) -> anyhow::Result<Box<dyn TransportConnection>> {
        let address = SocketAddr::from(([0, 0, 0, 0], port));
        let connecting = self.endpoint.connect(address, server_name)?;
        let connection = connecting.await?;

        Ok(Box::new(QUICConnection::new(connection)))
    }

    async fn accept_new_transport(&mut self) -> anyhow::Result<Box<dyn TransportConnection>> {
        let incoming = self.endpoint.accept().await.expect("failed to accept");
        let connection = incoming.await.expect("failed to create connection");

        Ok(Box::new(QUICConnection::new(connection)))
    }
}
