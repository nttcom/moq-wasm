use crate::{onvif_client::OnvifClient, onvif_profile_list};
use anyhow::{anyhow, Result};

pub struct ProfileTokens {
    pub profile_token: String,
    pub config_token: Option<String>,
}

pub async fn fetch(onvif: &OnvifClient) -> Result<ProfileTokens> {
    let profiles = onvif_profile_list::fetch(onvif).await?;
    let selected = profiles
        .first()
        .ok_or_else(|| anyhow!("Profiles not found"))?;
    Ok(ProfileTokens {
        profile_token: selected.token.clone(),
        config_token: selected.ptz_config.clone(),
    })
}
