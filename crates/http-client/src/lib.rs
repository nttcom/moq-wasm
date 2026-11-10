//! The workspace's `reqwest` clients: rustls with the ring crypto provider.
//! reqwest panics when it builds a client before a process crypto provider is
//! installed, so every client is built here, after installing one.

use reqwest::{Client, ClientBuilder};

/// Installs ring as the process's rustls crypto provider unless one is
/// already installed. Call it before a library builds its own client
/// (`object_store`, the OTLP exporter); `builder` and `client` call it.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[allow(clippy::disallowed_methods)]
pub fn builder() -> ClientBuilder {
    install_crypto_provider();
    Client::builder()
}

/// A client with reqwest's defaults; like `reqwest::Client::new`, panics when
/// the TLS backend cannot be initialized.
pub fn client() -> Client {
    builder()
        .build()
        .expect("reqwest cannot initialize rustls with the ring provider")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_client_without_a_provider_installed_beforehand() {
        // Arrange: this test binary installs no provider of its own.

        // Act
        let result = builder().build();

        // Assert
        assert!(result.is_ok());
    }
}
