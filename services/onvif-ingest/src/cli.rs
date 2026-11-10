use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "Control IP cameras via ONVIF PTZ")]
pub(crate) struct Args {
    /// IP address or hostname of the camera
    #[arg(long)]
    pub(crate) ip: String,

    /// Username for RTSP/ONVIF authentication
    #[arg(long)]
    pub(crate) username: String,

    /// Password for RTSP/ONVIF authentication
    #[arg(long)]
    pub(crate) password: String,

    /// Timeout in milliseconds
    #[arg(long, default_value_t = 3000)]
    pub(crate) timeout_ms: u64,
}
