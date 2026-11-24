use crate::onvif::{onvif_client::OnvifClient, onvif_requests, ptz_config::find_text};
use anyhow::{Result, anyhow};
use roxmltree::Document;

pub(crate) async fn fetch(onvif: &OnvifClient, profile_token: &str) -> Result<String> {
    let cmd = onvif_requests::get_stream_uri(profile_token);
    let body = onvif.send_media(&cmd).await?;
    extract_uri(&body)
}

fn extract_uri(body: &str) -> Result<String> {
    let doc = Document::parse(body).map_err(|err| anyhow!("invalid stream uri XML: {err}"))?;
    find_text(doc.root(), "Uri").ok_or_else(|| anyhow!("Stream URI not found"))
}
