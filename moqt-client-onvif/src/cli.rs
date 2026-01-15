use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OnvifAuth {
    Basic,
    Wsse,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Connect to IP cameras via RTSP and ONVIF")]
pub struct Args {
    /// IP address or hostname of the camera
    #[arg(long)]
    pub ip: String,

    /// Username for RTSP/ONVIF authentication
    #[arg(long)]
    pub username: Option<String>,

    /// Password for RTSP/ONVIF authentication
    #[arg(long)]
    pub password: Option<String>,

    /// RTSP port
    #[arg(long, default_value_t = 554)]
    pub rtsp_port: u16,

    /// RTSP path (e.g. /stream1)
    #[arg(long, default_value = "/")]
    pub rtsp_path: String,

    /// ONVIF port
    #[arg(long, default_value_t = 2020)]
    pub onvif_port: u16,

    /// ONVIF device service path
    #[arg(long, default_value = "/onvif/device_service")]
    pub onvif_path: String,

    /// ONVIF auth mode (basic or wsse)
    #[arg(long, value_enum, default_value_t = OnvifAuth::Basic)]
    pub onvif_auth: OnvifAuth,

    /// Allow invalid TLS certificates for ONVIF HTTPS
    #[arg(long)]
    pub onvif_insecure: bool,

    /// Enable interactive PTZ control with arrow keys
    #[arg(long)]
    pub ptz: bool,

    /// PTZ profile token override (auto-detect when omitted)
    #[arg(long)]
    pub ptz_profile_token: Option<String>,

    /// PTZ service endpoint override
    #[arg(long)]
    pub ptz_endpoint: Option<String>,

    /// Log PTZ SOAP responses
    #[arg(long)]
    pub ptz_log_responses: bool,

    /// Timeout in milliseconds
    #[arg(long, default_value_t = 3000)]
    pub timeout_ms: u64,
}
