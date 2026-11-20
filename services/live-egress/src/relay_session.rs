use std::sync::Arc;

use anyhow::{Context, Result};
use moqt::{ClientConfig, Endpoint, NATIVE, Session};

pub async fn connect(url: &str, config: &ClientConfig) -> Result<Arc<Session>> {
    let endpoint = Endpoint::<NATIVE>::create_client(config)?;
    tracing::info!(relay = %url, "connecting to relay");
    let connecting = endpoint
        .connect(url)
        .await
        .context("connect moqt transport")?;
    Ok(Arc::new(
        connecting.await.context("establish moqt session")?,
    ))
}
