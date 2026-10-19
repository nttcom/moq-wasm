use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};

use crate::{
    ServeArgs,
    api_server::ApiServer,
    launcher::Launcher,
    recordings::{Recordings, ServeConfig},
    store::RecordingStore,
};

/// Serves the recording API until SIGINT or SIGTERM. The recordings run as
/// jobs of their own and go on after the server exits.
pub(crate) async fn serve(args: ServeArgs) -> Result<()> {
    let store = RecordingStore::open(&args.store)?;
    let launcher = Launcher::from_args(&args.launcher)?;
    let recordings = Arc::new(Recordings::new(
        ServeConfig {
            namespace_root: args.namespace_root,
            max_recordings: args.max_recordings,
            composite: args.composite,
            store_url: args.store,
            output_dir: args.output_dir,
            room: args.room,
            launcher: args.launcher,
        },
        store,
        launcher,
    ));
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("cannot listen on {}", args.listen))?;
    let server = ApiServer::run(listener, recordings, args.allowed_origins);
    tracing::info!(listen = %args.listen, "recording API listening");
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
