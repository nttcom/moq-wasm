use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};

use crate::{
    ServeArgs,
    api_server::ApiServer,
    recorder::UPLOAD_SEGMENT_SECONDS,
    recordings::{Recordings, ServeConfig},
};

/// Serves the recording API until SIGINT or SIGTERM, then stops every
/// recording and waits for its files and uploads.
pub(crate) async fn serve(args: ServeArgs) -> Result<()> {
    let upload = args
        .upload
        .as_ref()
        .map(|url| {
            let (store, prefix) = object_store::parse_url_opts(url, std::env::vars())
                .with_context(|| format!("cannot use {url} as the upload location"))?;
            anyhow::Ok((Arc::from(store), prefix))
        })
        .transpose()?;
    tokio::fs::create_dir_all(&args.output_dir)
        .await
        .with_context(|| format!("cannot create {}", args.output_dir.display()))?;
    let recordings = Arc::new(Recordings::new(ServeConfig {
        relay: args.relay,
        verify_certificate: !args.insecure_skip_tls_verify,
        namespace_root: args.namespace_root,
        max_recordings: args.max_recordings,
        output_dir: args.output_dir,
        upload,
        max_video_height: args.max_video_height,
        segment_length: args
            .segment_seconds
            .or(args.upload.is_some().then_some(UPLOAD_SEGMENT_SECONDS))
            .map(Duration::from_secs),
    }));
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("cannot listen on {}", args.listen))?;
    let server = ApiServer::run(listener, Arc::clone(&recordings), args.allowed_origins);
    tracing::info!(listen = %args.listen, "recording API listening");
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    tracing::info!("shutting down; finishing the running recordings");
    drop(server);
    recordings.stop_all().await;
    Ok(())
}
