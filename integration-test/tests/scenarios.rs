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

    #[tokio::test]
    #[traced_test]
    async fn publish_namespace() -> Result<()> {
        // relayサーバーをバックグラウンドで起動
        let port_num = get_port();
        log_init();
        let (relay_shutdown_tx, relay_shutdown_rx) = oneshot::channel();
        let thread = run_relay_server(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await; // relayの起動を待つ

        // パスはプロジェクトのルートからの相対パス
        let client_cert_path = PathBuf::from("../keys/cert.pem");
        let _client_key_path = PathBuf::from("../keys/key.pem");

        let client = Client::new(
            client_cert_path.to_str().unwrap().to_string(),
            port_num,
            "Client A".to_string(),
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
        // relayバイナリのパス構築ロジックは削除
        let port_num = get_port();
        log_init();
        // relayサーバーをバックグラウンドで起動
        let (relay_shutdown_tx, relay_shutdown_rx) = oneshot::channel();
        let relay_handle = run_relay_server(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await; // relayの起動を待つ

        let client_cert_path = PathBuf::from("../keys/cert.pem");
        let _client_key_path = PathBuf::from("../keys/key.pem");

        // Client Aのインスタンス化と名前空間の公開
        let client_a = Client::new(
            client_cert_path.to_str().unwrap().to_string(),
            port_num,
            "Client A".to_string(),
        )
        .await?;
        let publish_namespace_a_result =
            client_a.publish_namespace("room/member".to_string()).await;
        assert!(
            publish_namespace_a_result.is_ok(),
            "Client A publish_namespace should return Ok"
        );

        // Client Bのインスタンス化と名前空間の購読
        let client_b = Client::new(
            client_cert_path.to_str().unwrap().to_string(),
            port_num,
            "Client B".to_string(),
        )
        .await?;
        let subscribe_namespace_b_result = client_b.subscribe_namespace("room".to_string()).await;
        assert!(
            subscribe_namespace_b_result.is_ok(),
            "Client B subscribe_namespace should return Ok"
        );

        tokio::time::sleep(Duration::from_secs(2)).await; // イベントが伝播する時間を確保

        // relayサーバーをシャットダウン
        let _ = relay_shutdown_tx.send(());
        let _ = relay_handle.await?; // relayタスクの終了を待つ

        // Client BがPublishNamespace通知を受け取ったことをアサート
        assert!(logs_contain(
            "Received: Client B Publish Namespace: room/member"
        ));

        Ok(())
    }
}
