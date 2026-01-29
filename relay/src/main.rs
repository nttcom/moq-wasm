use relay::run_relay_server;
use tokio::sync::oneshot; // oneshot::SenderとReceiverのために追加

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ロギングの初期化 (必要であれば)
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_line_number(true)
        .try_init()
        .ok();

    let (tx, rx) = oneshot::channel();

    // run_relay_serverをバックグラウンドで実行
    let relay_handle = tokio::spawn(async move {
        run_relay_server(4434, rx).await
    });

    tracing::info!("Ctrl+C to shutdown");
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown signal sent");
    let _ = tx.send(()); // シャットダウンシグナルを送信

    // relay_handleの終了を待つ
    relay_handle.await? // run_relay_serverのResultを待つ
        .map_err(|e| anyhow::anyhow!("Relay server failed: {}", e))?; // エラーを伝播

    tracing::info!("Relay server gracefully shutdown.");
    Ok(())
}