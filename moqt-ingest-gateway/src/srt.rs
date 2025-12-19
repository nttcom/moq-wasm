use anyhow::{Context, Result};
use futures::StreamExt;
use srt_tokio::{ConnectionRequest, SrtListener};
use tokio::task;

pub async fn run_srt_listener(addr: String) -> Result<()> {
    let (_listener, mut incoming) = SrtListener::builder()
        .bind(addr.as_str())
        .await
        .with_context(|| format!("bind SRT listener on {addr}"))?;
    println!("SRT listening on {addr}");

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
    println!("[srt {remote}] incoming stream_id={stream_id}");

    let result: Result<()> = async {
        let mut socket = request.accept(None).await?;
        let mut count = 0_u64;

        while let Some(packet) = socket.next().await {
            let (_, data) = packet?;
            count += 1;

            if count == 1 || count.is_multiple_of(200) {
                println!(
                    "[srt {remote}] packets={count} stream_id={stream_id} last_size={}",
                    data.len()
                );
            }
            // Not implemented: Forward data to MoQT server
        }

        println!("[srt {remote}] stream ended stream_id={stream_id} total_packets={count}");
        Ok(())
    }
    .await;

    if let Err(err) = result {
        eprintln!("[srt {remote}] error stream_id={stream_id}: {err:?}");
    }
}
