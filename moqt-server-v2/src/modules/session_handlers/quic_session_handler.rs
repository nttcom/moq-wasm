use anyhow::Ok;
use async_trait::async_trait;
use std::{net::SocketAddr, sync::Arc};

use quinn::{self, TransportConfig, VarInt};
use rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use tokio::sync::Mutex;

use crate::modules::session_handlers::{bi_stream::{BiStreamTrait, QuicBiStream}, session_handler_trait::SessionHandlerTrait};

struct QuicSessionHandler {
    _endpoint: quinn::Endpoint,
}

impl QuicSessionHandler {
    fn config_builder(
        cert_path: String,
        key_path: String,
        port_num: u16,
        keep_alive_sec: u64,
    ) -> anyhow::Result<quinn::ServerConfig> {
        let cert =
            vec![CertificateDer::from_pem_file(cert_path).expect("failed to load cert file.")];
        let key = PrivateKeyDer::from_pem_file(key_path).expect("failed to load key file.");

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert, key)?;
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

    pub fn new(
        cert_path: String,
        key_path: String,
        port_num: u16,
        keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        let server_config = Self::config_builder(cert_path, key_path, port_num, keep_alive_sec)
            .expect("failed to make configrattion");
        let address = SocketAddr::from(([0, 0, 0, 0], port_num));
        let endpoint = quinn::Endpoint::server(server_config, address)?;
        tracing::info!("Server ready! for QUIC");
        Ok(QuicSessionHandler {
            _endpoint: endpoint,
        })
    }
}

#[async_trait]
impl SessionHandlerTrait for QuicSessionHandler {
    async fn start(&self) -> anyhow::Result<Arc<Mutex<dyn BiStreamTrait>>> {
        let incoming = self._endpoint.accept().await.expect("failed to accept");
        let connection = incoming.await.expect("failed to create connection");
        let (control_send_stream, control_recv_stream) = connection
            .accept_bi()
            .await
            .expect("failed to accept bidirectional");
        let bi_stream = QuicBiStream::new(
            connection.stable_id(),
            control_recv_stream.id().into(),
            control_recv_stream,
            Arc::new(Mutex::new(control_send_stream)),
        );
        Ok(Arc::new(Mutex::new(bi_stream)))
    }

    fn finish(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
