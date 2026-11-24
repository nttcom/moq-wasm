use crate::pull::onvif::OnvifArgs;
use anyhow::{Result, bail};
use std::time::Duration;

const ONVIF_PORT: u16 = 2020;
const ONVIF_PATH: &str = "/onvif/device_service";

#[derive(Debug, Clone)]
pub(crate) struct Target {
    host: String,
    username: String,
    password: String,
    timeout: Duration,
}

impl Target {
    pub(crate) fn from_args(args: &OnvifArgs) -> Result<Self> {
        let host = args
            .onvif_ip
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string();
        if host.is_empty() {
            bail!("--onvif-ip is required");
        }
        let username = args
            .onvif_username
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string();
        if username.is_empty() {
            bail!("--onvif-username is required");
        }
        let password = args
            .onvif_password
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string();
        if password.is_empty() {
            bail!("--onvif-password is required");
        }
        Ok(Self {
            host,
            username,
            password,
            timeout: Duration::from_millis(args.onvif_timeout_ms),
        })
    }

    pub(crate) fn onvif_endpoint(&self) -> String {
        format!("http://{}:{}{}", self.host, ONVIF_PORT, ONVIF_PATH)
    }

    pub(crate) fn credentials(&self) -> (&str, &str) {
        (self.username.as_str(), self.password.as_str())
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }
}
