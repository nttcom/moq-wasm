use crate::{onvif_client::OnvifClient, onvif_requests, ptz_config::find_text, soap_client};
use anyhow::{anyhow, Result};
use roxmltree::Document;

pub struct ProfileSummary {
    pub token: String,
    pub name: Option<String>,
    pub video_source: Option<String>,
    pub video_encoder: Option<String>,
    pub ptz_config: Option<String>,
    pub video_resolution: Option<(u32, u32)>,
    pub video_framerate: Option<f64>,
}

pub async fn fetch(onvif: &OnvifClient) -> Result<Vec<ProfileSummary>> {
    let cmd = onvif_requests::get_profiles();
    let response = onvif.send_media(&cmd).await?;
    if response.status >= 400 {
        soap_client::log_response("GetProfiles", onvif.media_endpoint(), &response);
        return Err(anyhow!("get profiles failed with HTTP {}", response.status));
    }
    let profiles = extract_profiles(&response.body)?;
    log_profiles(&profiles);
    Ok(profiles)
}

fn extract_profiles(body: &str) -> Result<Vec<ProfileSummary>> {
    let doc = Document::parse(body).map_err(|err| anyhow!("invalid profiles XML: {err}"))?;
    let mut profiles = Vec::new();
    for profile in doc
        .descendants()
        .filter(|node| node.has_tag_name("Profiles"))
    {
        let token = match profile.attribute("token") {
            Some(token) => token.to_string(),
            None => {
                tracing::warn!("GetProfiles profile token missing");
                continue;
            }
        };
        let find_token = |tag: &str| {
            profile
                .descendants()
                .find(|node| node.has_tag_name(tag))
                .and_then(|node| node.attribute("token"))
                .map(str::to_string)
        };
        let encoder = profile
            .descendants()
            .find(|node| node.has_tag_name("VideoEncoderConfiguration"));
        let video_resolution = encoder.and_then(|encoder| {
            let width = find_text(encoder, "Width")?.parse::<u32>().ok()?;
            let height = find_text(encoder, "Height")?.parse::<u32>().ok()?;
            Some((width, height))
        });
        let video_framerate = encoder.and_then(|encoder| {
            encoder
                .descendants()
                .find(|n| n.has_tag_name("FrameRateLimit") || n.has_tag_name("FrameRate"))
                .and_then(|node| node.text())
                .and_then(|text| text.trim().parse::<f64>().ok())
        });
        profiles.push(ProfileSummary {
            token,
            name: find_text(profile, "Name"),
            video_source: find_token("VideoSourceConfiguration"),
            video_encoder: find_token("VideoEncoderConfiguration"),
            ptz_config: find_token("PTZConfiguration"),
            video_resolution,
            video_framerate,
        });
    }
    if profiles.is_empty() {
        return Err(anyhow!("Profiles not found"));
    }
    Ok(profiles)
}

fn log_profiles(profiles: &[ProfileSummary]) {
    tracing::info!("[GetProfiles] profiles={}", profiles.len());
    for (index, profile) in profiles.iter().enumerate() {
        let name = profile.name.as_deref().unwrap_or("-");
        let video_source = profile.video_source.as_deref().unwrap_or("-");
        let video_encoder = profile.video_encoder.as_deref().unwrap_or("-");
        let ptz_config = profile.ptz_config.as_deref().unwrap_or("-");
        let resolution = profile
            .video_resolution
            .map(|(width, height)| format!("{width}x{height}"))
            .unwrap_or_else(|| "-".to_string());
        let framerate = profile
            .video_framerate
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "-".to_string());
        tracing::info!(
            "  [{}] token={} name={} video_source={} video_encoder={} ptz_config={} resolution={} framerate={}",
            index,
            profile.token,
            name,
            video_source,
            video_encoder,
            ptz_config,
            resolution,
            framerate
        );
    }
}
