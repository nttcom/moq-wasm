use crate::cli::{Args, OnvifAuth};
use anyhow::{bail, Result};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Target {
    host: String,
    username: Option<String>,
    password: Option<String>,
    rtsp_port: u16,
    rtsp_path: String,
    onvif_port: u16,
    onvif_path: String,
    onvif_auth: OnvifAuth,
    onvif_insecure: bool,
    timeout: Duration,
}

impl Target {
    pub fn from_args(args: &Args) -> Result<Self> {
        let host = args.ip.trim().to_string();
        if host.is_empty() {
            bail!("ip is required");
        }
        Ok(Self {
            host,
            username: args.username.clone(),
            password: args.password.clone(),
            rtsp_port: args.rtsp_port,
            rtsp_path: normalize_path(&args.rtsp_path),
            onvif_port: args.onvif_port,
            onvif_path: normalize_path(&args.onvif_path),
            onvif_auth: args.onvif_auth,
            onvif_insecure: args.onvif_insecure,
            timeout: Duration::from_millis(args.timeout_ms),
        })
    }

    pub fn rtsp_endpoint(&self) -> String {
        let credentials = match (&self.username, &self.password) {
            (Some(user), Some(pass)) => format!("{}:{}@", user, pass),
            _ => String::new(),
        };
        format!(
            "rtsp://{}{}:{}{}",
            credentials, self.host, self.rtsp_port, self.rtsp_path
        )
    }

    pub fn rtsp_display(&self) -> String {
        format!("rtsp://{}:{}{}", self.host, self.rtsp_port, self.rtsp_path)
    }

    pub fn rtsp_socket_addr(&self) -> String {
        format!("{}:{}", self.host, self.rtsp_port)
    }

    pub fn onvif_endpoint(&self) -> String {
        format!(
            "http://{}:{}{}",
            self.host, self.onvif_port, self.onvif_path
        )
    }

    pub fn onvif_auth(&self) -> OnvifAuth {
        self.onvif_auth
    }

    pub fn onvif_insecure(&self) -> bool {
        self.onvif_insecure
    }

    pub fn basic_auth(&self) -> Option<(&str, &str)> {
        match (&self.username, &self.password) {
            (Some(user), Some(pass)) => Some((user.as_str(), pass.as_str())),
            _ => None,
        }
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return "/".to_string();
    }
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{}", trimmed)
    }
}
