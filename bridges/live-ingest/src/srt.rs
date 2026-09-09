use anyhow::{Context, Result};
use futures::StreamExt;
use mediapack::mpegts;
use srt_tokio::{ConnectionRequest, SrtListener};
use tokio::task;

use crate::{
    moqt::MoqtManager,
    publisher::{IngestOptions, MediaPublisher},
};

const DEFAULT_NAMESPACE: &str = "srt/live";
const ACCESS_CONTROL_PREFIX: &str = "#!::";

pub async fn run_srt_listener(addr: String, options: IngestOptions) -> Result<()> {
    let (_listener, mut incoming) = SrtListener::builder()
        .bind(addr.as_str())
        .await
        .with_context(|| format!("bind SRT listener on {addr}"))?;
    tracing::info!(%addr, "SRT listener started");
    while let Some(request) = incoming.incoming().next().await {
        task::spawn(handle_request(request, options.clone()));
    }
    Ok(())
}

async fn handle_request(request: ConnectionRequest, options: IngestOptions) {
    let stream_id = request.stream_id().map(ToString::to_string);
    let namespace = namespace_from_stream_id(stream_id.as_deref());
    let remote = request.remote();
    tracing::info!(%remote, ?stream_id, namespace = %namespace.join("/"), "SRT publisher connected");
    match publish(request, &options, namespace).await {
        Ok(packets) => tracing::info!(%remote, packets, "SRT stream ended"),
        Err(err) => tracing::warn!(%remote, ?err, "SRT publisher failed"),
    }
}

async fn publish(
    request: ConnectionRequest,
    options: &IngestOptions,
    namespace: Vec<String>,
) -> Result<u64> {
    let mut socket = request.accept(None).await?;
    let mut demuxer = mpegts::Demuxer::new();
    let mut publisher = MediaPublisher::new(
        MoqtManager::new(options.moqt.clone()),
        namespace,
        options.transcode,
    );
    let mut packets = 0_u64;
    while let Some(packet) = socket.next().await {
        let (_, data) = packet?;
        packets += 1;
        for event in demuxer.push(&data)? {
            publisher.push(&event).await?;
        }
    }
    for event in demuxer.finish()? {
        publisher.push(&event).await?;
    }
    Ok(packets)
}

fn namespace_from_stream_id(stream_id: Option<&str>) -> Vec<String> {
    let resource = match stream_id {
        Some(control) if control.starts_with(ACCESS_CONTROL_PREFIX) => control
            .strip_prefix(ACCESS_CONTROL_PREFIX)
            .and_then(|fields| fields.split(',').find_map(|field| field.strip_prefix("r=")))
            .unwrap_or(DEFAULT_NAMESPACE),
        Some(plain) => plain,
        None => DEFAULT_NAMESPACE,
    };
    let parts: Vec<String> = resource
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    if parts.is_empty() {
        namespace_from_stream_id(Some(DEFAULT_NAMESPACE))
    } else {
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_srt_access_control_resource() {
        // Arrange
        let stream_id = "#!::r=live/camera-1,m=publish";

        // Act
        let namespace = namespace_from_stream_id(Some(stream_id));

        // Assert
        assert_eq!(namespace, ["live", "camera-1"]);
    }

    #[test]
    fn accepts_plain_stream_id() {
        // Arrange
        let stream_id = "live/camera-2";

        // Act
        let namespace = namespace_from_stream_id(Some(stream_id));

        // Assert
        assert_eq!(namespace, ["live", "camera-2"]);
    }

    #[test]
    fn falls_back_when_stream_id_is_missing_or_empty() {
        // Arrange / Act
        let missing = namespace_from_stream_id(None);
        let empty = namespace_from_stream_id(Some(""));
        let without_resource = namespace_from_stream_id(Some("#!::m=publish"));

        // Assert
        assert_eq!(missing, ["srt", "live"]);
        assert_eq!(empty, ["srt", "live"]);
        assert_eq!(without_resource, ["srt", "live"]);
    }
}
