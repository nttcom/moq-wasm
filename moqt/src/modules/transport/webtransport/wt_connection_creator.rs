use std::net::SocketAddr;

use async_trait::async_trait;

use super::wt_connection::WtConnection;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;

enum WtEndpoint {
    Server(tokio::sync::Mutex<web_transport_quinn::Server>),
    Client(web_transport_quinn::Client),
}

pub struct WtConnectionCreator {
    endpoint: WtEndpoint,
}

#[async_trait]
impl TransportConnectionCreator for WtConnectionCreator {
    type Connection = WtConnection;

    fn client(port_num: u16, verify_certificate: bool) -> anyhow::Result<Self> {
        let client = if verify_certificate {
            web_transport_quinn::ClientBuilder::new().with_system_roots()?
        } else {
            web_transport_quinn::ClientBuilder::new()
                .dangerous()
                .with_no_certificate_verification()?
        };

        tracing::info!("Client ready! for WebTransport port: {}", port_num);

        Ok(WtConnectionCreator {
            endpoint: WtEndpoint::Client(client),
        })
    }

    fn client_with_custom_cert(port_num: u16, custom_cert_path: &str) -> anyhow::Result<Self> {
        use std::fs::File;
        use std::io::BufReader;

        // 証明書を読み込む
        let cert_file = File::open(custom_cert_path)
            .inspect_err(|e| tracing::error!("Opening certificate file failed: {:?}", e))?;
        let mut cert_reader = BufReader::new(cert_file);
        let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .inspect_err(|e| tracing::error!("Parsing certificate failed: {:?}", e))?;

        // 自己署名証明書を信頼するクライアントを作る
        let client = web_transport_quinn::ClientBuilder::new().with_server_certificates(certs)?;

        tracing::info!("Client ready! for WebTransport port: {}", port_num);

        Ok(WtConnectionCreator {
            endpoint: WtEndpoint::Client(client),
        })
    }

    fn server(
        cert_path: &str,
        key_path: &str,
        port_num: u16,
        _keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        use std::fs::File;
        use std::io::BufReader;

        // 証明書を同期的に読み込む
        let cert_file = File::open(cert_path)
            .inspect_err(|e| tracing::error!("Opening certificate file failed: {:?}", e))?;
        let mut cert_reader = BufReader::new(cert_file);
        let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .inspect_err(|e| tracing::error!("Parsing certificates failed: {:?}", e))?;

        // 秘密鍵を同期的に読み込む
        let key_file = File::open(key_path)
            .inspect_err(|e| tracing::error!("Opening key file failed: {:?}", e))?;
        let mut key_reader = BufReader::new(key_file);
        let key = rustls_pemfile::private_key(&mut key_reader)?
            .ok_or_else(|| anyhow::anyhow!("No private key found"))?;

        let addr: SocketAddr = format!("[::]:{}", port_num).parse()?;
        let server = web_transport_quinn::ServerBuilder::new()
            .with_addr(addr)
            .with_certificate(certs, key)?;

        tracing::info!("Server ready! for WebTransport port: {}", port_num);

        Ok(WtConnectionCreator {
            endpoint: WtEndpoint::Server(tokio::sync::Mutex::new(server)),
        })
    }

    async fn create_new_transport(
        &self,
        remote_address: SocketAddr,
        host: &str,
    ) -> anyhow::Result<Self::Connection> {
        let client = match &self.endpoint {
            WtEndpoint::Client(c) => c,
            WtEndpoint::Server(_) => {
                anyhow::bail!("Cannot create_new_transport on a server endpoint")
            }
        };
        let url: url::Url = format!("https://{}:{}", host, remote_address.port()).parse()?;

        let session = client
            .connect(url)
            .await
            .inspect_err(|e| tracing::error!("failed to connect: {:?}", e))?;

        Ok(WtConnection::new(session))
    }

    async fn accept_new_transport(&mut self) -> anyhow::Result<Self::Connection> {
        let server = match &self.endpoint {
            WtEndpoint::Server(s) => s,
            WtEndpoint::Client(_) => {
                anyhow::bail!("Cannot accept_new_transport on a client endpoint")
            }
        };

        // クライアントの接続を待つ
        let Some(request) = server.lock().await.accept().await else {
            anyhow::bail!("Server endpoint closed");
        };

        // リクエストを受け入れてセッションを確立する
        let session = request
            .ok()
            .await
            .inspect_err(|e| tracing::error!("failed to establish session: {:?}", e))?;

        Ok(WtConnection::new(session))
    }
}
