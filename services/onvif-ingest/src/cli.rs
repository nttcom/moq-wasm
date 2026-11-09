use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "Control IP cameras via ONVIF PTZ")]
pub struct Args {
    /// IP address or hostname of the camera
    #[arg(long)]
    pub ip: String,

    /// Username for RTSP/ONVIF authentication
    #[arg(long)]
    pub username: String,

    /// Password for RTSP/ONVIF authentication
    #[arg(long)]
    pub password: String,

    /// Timeout in milliseconds
    #[arg(long, default_value_t = 3000)]
    pub timeout_ms: u64,
}
