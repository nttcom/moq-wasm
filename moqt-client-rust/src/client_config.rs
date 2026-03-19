use anyhow::Result;
use std::sync::Arc;
use wtransport::tls::build_native_cert_store;
use wtransport::tls::client::{build_default_tls_config, NoServerVerification};
use wtransport::ClientConfig;

pub(crate) async fn build_client_config(insecure_skip_tls_verify: bool) -> Result<ClientConfig> {
    if insecure_skip_tls_verify {
        let tls_config = build_default_tls_config(
            Arc::new(build_native_cert_store()),
            Some(Arc::new(NoServerVerification::new())),
        );
        return Ok(ClientConfig::builder()
            .with_bind_default()
            .with_custom_tls(tls_config)
            .build());
    }

    Ok(ClientConfig::default())
}
