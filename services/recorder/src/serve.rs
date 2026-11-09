use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};

use crate::{
    ServeArgs, api_server::ApiServer, launcher::Launcher, recordings::Recordings,
    store::RecordingStore,
};

/// Serves the recording API until SIGINT or SIGTERM. The recordings run as
/// jobs of their own and go on after the server exits.
pub(crate) async fn serve(mut args: ServeArgs) -> Result<()> {
    let store = RecordingStore::open(&args.store)?;
    let launcher = Launcher::from_args(&args.launcher)?;
    let listen = args.listen;
    let allowed_origins = std::mem::take(&mut args.allowed_origins);
    let recordings = Arc::new(Recordings::new(args, store, launcher));
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("cannot listen on {listen}"))?;
    let server = ApiServer::run(listener, recordings, allowed_origins);
    tracing::info!(listen = %listen, "recording API listening");
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    tracing::info!("shutting down");
    drop(server);
    Ok(())
}
