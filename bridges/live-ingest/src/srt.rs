use anyhow::{Context, Result};
use futures::StreamExt;
use srt_tokio::{ConnectionRequest, SrtListener};
use tokio::task;

pub async fn run_srt_listener(addr: String) -> Result<()> {
    let (_listener, mut incoming) = SrtListener::builder()
        .bind(addr.as_str())
        .await
        .with_context(|| format!("bind SRT listener on {addr}"))?;
    tracing::info!(%addr, "SRT listener started");

    while let Some(request) = incoming.incoming().next().await {
        task::spawn(handle_request(request));
    }

    Ok(())
}

async fn handle_request(request: ConnectionRequest) {
    let stream_id = request
        .stream_id()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "<no-streamid>".to_string());
    let remote = request.remote();
    tracing::info!(%remote, %stream_id, "SRT caller connected");

    let result: Result<()> = async {
        let mut socket = request.accept(None).await?;
        let mut count = 0_u64;

        while let Some(packet) = socket.next().await {
            let (_, data) = packet?;
            count += 1;

            if count == 1 || count.is_multiple_of(200) {
                tracing::debug!(%remote, %stream_id, packets = count, last_size = data.len(), "SRT packets received");
            }
            // Not implemented: Forward data to MoQT server
        }

        tracing::info!(%remote, %stream_id, total_packets = count, "SRT stream ended");
        Ok(())
    }
    .await;

    if let Err(err) = result {
        tracing::warn!(%remote, %stream_id, ?err, "SRT connection failed");
    }
}
