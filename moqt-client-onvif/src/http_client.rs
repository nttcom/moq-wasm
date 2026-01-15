use crate::config::Target;
use anyhow::{Context, Result};
use reqwest::Client;

pub fn build(target: &Target) -> Result<Client> {
    Client::builder()
        .timeout(target.timeout())
        .danger_accept_invalid_certs(target.onvif_insecure())
        .build()
        .context("http client build failed")
}
