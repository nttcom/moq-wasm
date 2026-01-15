use crate::config::Target;
use anyhow::{Context, Result};
use reqwest::Client;

pub fn build(target: &Target) -> Result<Client> {
    Client::builder()
        .timeout(target.timeout())
        .build()
        .context("http client build failed")
}
