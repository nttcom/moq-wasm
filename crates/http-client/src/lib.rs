//! Builds the workspace's `reqwest` clients.

use reqwest::{Client, ClientBuilder};

#[allow(clippy::disallowed_methods)]
pub fn builder() -> ClientBuilder {
    Client::builder()
}

/// A client with reqwest's defaults; like `reqwest::Client::new`, panics when
/// the TLS backend cannot be initialized.
pub fn client() -> Client {
    builder()
        .build()
        .expect("reqwest cannot initialize its TLS backend")
}
