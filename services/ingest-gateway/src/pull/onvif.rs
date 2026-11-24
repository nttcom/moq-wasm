mod app_config;
mod command_subscriber;
mod onvif_client;
mod onvif_profile_list;
mod onvif_requests;
mod onvif_services;
mod onvif_stream_uri;
mod ptz_config;
mod ptz_worker;
mod soap_client;
mod wsse_auth;

use anyhow::{Result, anyhow, bail};
use moqt::client::Session;

pub(super) use self::app_config::Target;
pub(super) use self::command_subscriber::CommandSubscriber;
use self::command_subscriber::CommandTrack;

#[derive(clap::Args, Debug)]
pub(crate) struct OnvifArgs {
    /// IP address or hostname of an ONVIF camera: publish its media profiles
    /// (unless --rtsp-url is given) and apply the PTZ commands from --command-track to it
    #[arg(long, requires_all = ["onvif_username", "onvif_password"])]
    pub(crate) onvif_ip: Option<String>,

    /// Username for ONVIF and RTSP authentication
    #[arg(long)]
    pub(crate) onvif_username: Option<String>,

    /// Password for ONVIF and RTSP authentication
    #[arg(long)]
    pub(crate) onvif_password: Option<String>,

    /// Timeout of an ONVIF request in milliseconds
    #[arg(long, default_value_t = 3000)]
    pub(crate) onvif_timeout_ms: u64,

    /// Track namespace the PTZ commands are subscribed under (slash-separated)
    #[arg(long, default_value = "anon/onvif/viewer")]
    command_namespace: String,

    /// Track name of the PTZ commands
    #[arg(long, default_value = "command")]
    command_track: String,
}

/// `rtsp_url` carries the camera's credentials.
pub(super) struct CameraProfile {
    pub(super) token: String,
    pub(super) label: String,
    pub(super) rtsp_url: String,
}

impl OnvifArgs {
    pub(super) fn target(&self) -> Result<Option<Target>> {
        self.onvif_ip
            .as_ref()
            .map(|_| Target::from_args(self))
            .transpose()
    }
}

/// Skips a profile whose GetStreamUri fails; fails when no profile is left.
pub(super) async fn fetch_profiles(target: &Target) -> Result<Vec<CameraProfile>> {
    let client = soap_client::build(target)?;
    let onvif = onvif_client::OnvifClient::initialize_media(client, target.clone()).await?;
    let profiles = onvif_profile_list::fetch(&onvif).await?;
    let mut camera_profiles = Vec::new();
    for profile in profiles {
        let uri = match onvif_stream_uri::fetch(&onvif, &profile.token).await {
            Ok(uri) => uri,
            Err(err) => {
                tracing::warn!(
                    "GetStreamUri failed for profile_token={}: {err}",
                    profile.token
                );
                continue;
            }
        };
        camera_profiles.push(CameraProfile {
            label: profile
                .name
                .clone()
                .unwrap_or_else(|| profile.token.clone()),
            rtsp_url: apply_rtsp_credentials(&uri, target),
            token: profile.token,
        });
    }
    if camera_profiles.is_empty() {
        return Err(anyhow!("no usable video profiles"));
    }
    Ok(camera_profiles)
}

/// Starts the PTZ worker and follows the command track on `session`.
pub(super) async fn follow_commands(
    session: &Session,
    args: &OnvifArgs,
    target: Target,
) -> Result<CommandSubscriber> {
    let namespace = super::parse_namespace(&args.command_namespace);
    if namespace.is_empty() {
        bail!("command namespace is required");
    }
    CommandSubscriber::run(
        session,
        CommandTrack {
            namespace,
            track: args.command_track.clone(),
        },
        ptz_worker::spawn(target),
    )
    .await
}

fn apply_rtsp_credentials(uri: &str, target: &Target) -> String {
    let (username, password) = target.credentials();
    for prefix in ["rtsp://", "rtsps://"] {
        if let Some(rest) = uri.strip_prefix(prefix) {
            if rest.contains('@') {
                return uri.to_string();
            }
            return format!("{prefix}{username}:{password}@{rest}");
        }
    }
    uri.to_string()
}
