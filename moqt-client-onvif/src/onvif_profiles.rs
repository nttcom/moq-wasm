use crate::{onvif_client::OnvifClient, onvif_requests, soap_client};
use anyhow::{anyhow, Result};
use roxmltree::Document;

pub struct ProfileTokens {
    pub profile_token: String,
    pub config_token: Option<String>,
}

pub async fn fetch(onvif: &OnvifClient) -> Result<ProfileTokens> {
    let cmd = onvif_requests::get_profiles();
    let response = onvif.send_media(&cmd).await?;
    if response.status >= 400 {
        soap_client::log_response("GetProfiles", onvif.media_endpoint(), &response);
        return Err(anyhow!("get profiles failed with HTTP {}", response.status));
    }
    let (profile_token, config_token) = extract_profile_tokens(&response.body)?;
    Ok(ProfileTokens {
        profile_token,
        config_token,
    })
}

fn extract_profile_tokens(body: &str) -> Result<(String, Option<String>)> {
    let doc = Document::parse(body).map_err(|err| anyhow!("invalid profiles XML: {err}"))?;
    let profile = doc
        .descendants()
        .find(|node| node.has_tag_name("Profiles"))
        .ok_or_else(|| anyhow!("Profiles not found"))?;
    let profile_token = profile
        .attribute("token")
        .ok_or_else(|| anyhow!("Profile token missing"))?;
    let config_token = profile
        .descendants()
        .find(|node| node.has_tag_name("PTZConfiguration"))
        .and_then(|node| node.attribute("token"))
        .map(str::to_string);
    Ok((profile_token.to_string(), config_token))
}
