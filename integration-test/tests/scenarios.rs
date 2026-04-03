mod integration_test {
    use anyhow::Result;
    use integration_test::{
        Client,
        util::scenarios::{
            PublisherServeMode, TEST_TRACK_NAMESPACE, activate_server,
            assert_receive_datagram_objects, assert_receive_reopened_streams,
            assert_receive_stream_objects, connect_session_quic, get_cert_path, get_port,
            serve_publisher_side,
        },
    };
    use std::time::Duration;

    use tokio::sync::oneshot; // oneshot::SenderとReceiverのために追加
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn publish_namespace() -> Result<()> {
        let port_num = get_port();
        let (relay_shutdown_tx, relay_shutdown_rx) = oneshot::channel();
        activate_server::<moqt::QUIC>(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await; // relayの起動を待つ

        let client = Client::<moqt::QUIC>::new(
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
        let relay_handle = activate_server::<moqt::QUIC>(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await; // relayの起動を待つ

        // Client Aのインスタンス化と名前空間の公開
        let client_a = Client::<moqt::QUIC>::new(
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
        let client_b = Client::<moqt::QUIC>::new(
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
        assert!(
            received_namespace.is_ok(),
            "Did not receive notification in time"
        );
        assert_eq!(
            received_namespace.unwrap().unwrap(),
            "room/member".to_string()
        );

        // relayサーバーをシャットダウン
        let _ = relay_shutdown_tx.send(());
        relay_handle.await?; // relayタスクの終了を待つ

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn wt_publish_namespace() -> Result<()> {
        let port_num = get_port();
        let (relay_shutdown_tx, relay_shutdown_rx) = oneshot::channel();
        activate_server::<moqt::WEBTRANSPORT>(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let client = Client::<moqt::WEBTRANSPORT>::new(
            get_cert_path().to_str().unwrap().to_string(),
            port_num,
            "Client A".to_string(),
            None,
        )
        .await?;
        let result = client.publish_namespace("room/member".to_string()).await;
        assert!(result.is_ok(), "wt publish_namespace should return Ok");

        tokio::time::sleep(Duration::from_secs(1)).await;

        // relayサーバーをシャットダウン
        let _ = relay_shutdown_tx.send(());

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn wt_publish_namespace_already_subscribe_namespace() -> Result<()> {
        let port_num = get_port();
        let (relay_shutdown_tx, relay_shutdown_rx) = oneshot::channel();
        let relay_handle = activate_server::<moqt::WEBTRANSPORT>(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Client Aのインスタンス化と名前空間の公開
        let client_a = Client::<moqt::WEBTRANSPORT>::new(
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
        let client_b = Client::<moqt::WEBTRANSPORT>::new(
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
        assert!(
            received_namespace.is_ok(),
            "Did not receive notification in time"
        );
        assert_eq!(
            received_namespace.unwrap().unwrap(),
            "room/member".to_string()
        );

        // relayサーバーをシャットダウン
        let _ = relay_shutdown_tx.send(());
        relay_handle.await?;

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn stream_receive_five_objects_via_relay() -> Result<()> {
        let port_num = get_port();
        let (relay_shutdown_tx, relay_shutdown_rx) = oneshot::channel();
        let relay_handle = activate_server::<moqt::QUIC>(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let publisher_session = connect_session_quic(port_num).await?;
        let publisher = publisher_session.publisher();
        publisher
            .publish_namespace(TEST_TRACK_NAMESPACE.to_string())
            .await?;
        let serve_task = tokio::spawn(serve_publisher_side(
            publisher_session,
            "stream-five-objects".to_string(),
            PublisherServeMode::StreamFiveObjects,
        ));

        let subscriber_session = connect_session_quic(port_num).await?;
        assert_receive_stream_objects(subscriber_session.subscriber(), "stream-five-objects", 5)
            .await?;

        serve_task.await??;
        let _ = relay_shutdown_tx.send(());
        relay_handle.await?;
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn datagram_receive_five_objects_via_relay() -> Result<()> {
        let port_num = get_port();
        let (relay_shutdown_tx, relay_shutdown_rx) = oneshot::channel();
        let relay_handle = activate_server::<moqt::QUIC>(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let publisher_session = connect_session_quic(port_num).await?;
        let publisher = publisher_session.publisher();
        publisher
            .publish_namespace(TEST_TRACK_NAMESPACE.to_string())
            .await?;
        let serve_task = tokio::spawn(serve_publisher_side(
            publisher_session,
            "datagram-five-objects".to_string(),
            PublisherServeMode::DatagramFiveObjects,
        ));

        let subscriber_session = connect_session_quic(port_num).await?;
        assert_receive_datagram_objects(
            subscriber_session.subscriber(),
            "datagram-five-objects",
            5,
        )
        .await?;

        serve_task.await??;
        let _ = relay_shutdown_tx.send(());
        relay_handle.await?;
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn stream_reopen_five_times_via_relay() -> Result<()> {
        let port_num = get_port();
        let (relay_shutdown_tx, relay_shutdown_rx) = oneshot::channel();
        let relay_handle = activate_server::<moqt::QUIC>(port_num, relay_shutdown_rx);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let publisher_session = connect_session_quic(port_num).await?;
        let publisher = publisher_session.publisher();
        publisher
            .publish_namespace(TEST_TRACK_NAMESPACE.to_string())
            .await?;
        let serve_task = tokio::spawn(serve_publisher_side(
            publisher_session,
            "stream-reopen-five-times".to_string(),
            PublisherServeMode::StreamReopenFiveTimes,
        ));

        let subscriber_session = connect_session_quic(port_num).await?;
        assert_receive_reopened_streams(
            subscriber_session.subscriber(),
            "stream-reopen-five-times",
            5,
        )
        .await?;

        serve_task.await??;
        let _ = relay_shutdown_tx.send(());
        relay_handle.await?;
        Ok(())
    }
}
