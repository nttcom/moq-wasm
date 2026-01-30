mod integration_test {

    use anyhow::Result;
    use integration_test::Client;
    use std::path::PathBuf;
    use std::time::Duration;

    use relay::run_relay_server; // relayクレートのrun_relay_server関数をインポート
    use tokio::sync::oneshot; // oneshot::SenderとReceiverのために追加
    use tracing_test::traced_test;

    static PORT_NUMBER: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(1000);
    static LOG_INITIALIZER: std::sync::Once = std::sync::Once::new();

    fn get_port() -> u16 {
        PORT_NUMBER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    fn log_init() {
        LOG_INITIALIZER.call_once(|| {
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .with_line_number(true)
                .try_init()
                .ok();
        });
    }

    fn get_cert_path() -> PathBuf {
        let current = std::env::current_dir().unwrap();
        current.join("keys").join("cert.pem")
    }

    fn get_key_path() -> PathBuf {
        let current = std::env::current_dir().unwrap();
        current.join("keys").join("key.pem")
    }

    fn activate_server(
        port_num: u16,
        receiver: tokio::sync::oneshot::Receiver<()>,
    ) -> tokio::task::JoinHandle<()> {
        let key_path = get_key_path();
        let cert_path = get_cert_path();
        log_init();
        run_relay_server(
            port_num,
            receiver,
            key_path.to_str().unwrap(),
            cert_path.to_str().unwrap(),
        )
    }

    #[tokio::test]
    #[traced_test]
    async fn publish_namespace() -> Result<()> {
        let port_num = get_port();
        let (relay_shutdown_tx, relay_shutdown_rx) = oneshot::channel();
        activate_server(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await; // relayの起動を待つ

        let client = Client::new(
            get_cert_path().to_str().unwrap().to_string(),
            port_num,
            "Client A".to_string(),
            None,
        )
        .await?;
        let result = client.publish_namespace("room/member".to_string()).await;
        assert!(result.is_ok(), "publish_namespace should return Ok");

        tokio::time::sleep(Duration::from_secs(1)).await; // publishが登録される時間を確保

        // relayサーバーをシャットダウン
        let _ = relay_shutdown_tx.send(());

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn publish_namespace_already_subscribe_namespace() -> Result<()> {
        let port_num = get_port();
        let (relay_shutdown_tx, relay_shutdown_rx) = oneshot::channel();
        let relay_handle = activate_server(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await; // relayの起動を待つ

        // Client Aのインスタンス化と名前空間の公開
        let client_a = Client::new(
            get_cert_path().to_str().unwrap().to_string(),
            port_num,
            "Client A".to_string(),
            None,
        )
        .await?;
        let publish_namespace_a_result =
            client_a.publish_namespace("room/member".to_string()).await;
        assert!(
            publish_namespace_a_result.is_ok(),
            "Client A publish_namespace should return Ok"
        );

        // Client Bのインスタンス化と名前空間の購読
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let client_b = Client::new(
            get_cert_path().to_str().unwrap().to_string(),
            port_num,
            "Client B".to_string(),
            Some(tx),
        )
        .await?;
        let subscribe_namespace_b_result = client_b.subscribe_namespace("room".to_string()).await;
        assert!(
            subscribe_namespace_b_result.is_ok(),
            "Client B subscribe_namespace should return Ok"
        );

        // Client BがPublishNamespace通知を受け取ったことをアサート
        let received_namespace = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(received_namespace.is_ok(), "Did not receive notification in time");
        assert_eq!(
            received_namespace.unwrap().unwrap(),
            "room/member".to_string()
        );

        // relayサーバーをシャットダウン
        let _ = relay_shutdown_tx.send(());
        relay_handle.await?; // relayタスクの終了を待つ

        Ok(())
    }
}
