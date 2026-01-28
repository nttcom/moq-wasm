use crate::cli::Args;
use anyhow::{bail, Result};
use std::time::Duration;

const ONVIF_PORT: u16 = 2020;
const ONVIF_PATH: &str = "/onvif/device_service";
const RTSP_PORT: u16 = 554;
const RTSP_PATH: &str = "/stream1";

#[derive(Debug, Clone)]
pub struct Target {
    host: String,
    username: String,
    password: String,
    timeout: Duration,
}

impl Target {
    pub fn from_args(args: &Args) -> Result<Self> {
        let host = args.ip.trim().to_string();
        if host.is_empty() {
            bail!("ip is required");
        }
        let username = args.username.trim().to_string();
        if username.is_empty() {
            bail!("username is required");
        }
        let password = args.password.trim().to_string();
        if password.is_empty() {
            bail!("password is required");
        }
        Ok(Self {
            host,
            username,
            password,
            timeout: Duration::from_millis(args.timeout_ms),
        })
    }

    pub fn onvif_endpoint(&self) -> String {
        format!("http://{}:{}{}", self.host, ONVIF_PORT, ONVIF_PATH)
    }

    pub fn rtsp_url(&self) -> String {
        format!(
            "rtsp://{}:{}@{}:{}{}",
            self.username, self.password, self.host, RTSP_PORT, RTSP_PATH
        )
    }

    pub fn credentials(&self) -> (&str, &str) {
        (self.username.as_str(), self.password.as_str())
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}
