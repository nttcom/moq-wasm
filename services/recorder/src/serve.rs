use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::TcpListener;

use crate::{ServeArgs, api_server::ApiServer, recordings::Recordings, store::RecordingStore};

/// Serves the recording API until SIGINT or SIGTERM. The recordings run as
/// jobs `bot-dispatcher` starts and go on after the server exits.
pub(crate) async fn serve(mut args: ServeArgs) -> Result<()> {
    let store = RecordingStore::open(&args.store)?;
    let listen = args.listen;
    let allowed_origins = std::mem::take(&mut args.allowed_origins);
    let recordings = Arc::new(Recordings::new(args, store));
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("cannot listen on {listen}"))?;
    let server = ApiServer::run(listener, recordings, allowed_origins);
    tracing::info!(listen = %listen, "recording API listening");
    http_server::shutdown_signal().await?;
    tracing::info!("shutting down");
    drop(server);
    Ok(())
}
