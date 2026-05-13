use crate::{onvif_client::OnvifClient, onvif_requests, soap_client};
use anyhow::{anyhow, Result};
use roxmltree::Document;

pub async fn fetch(onvif: &OnvifClient, profile_token: &str) -> Result<String> {
    let cmd = onvif_requests::get_stream_uri(profile_token);
    let response = onvif.send_media(&cmd).await?;
    if response.status >= 400 {
        soap_client::log_response("GetStreamUri", onvif.media_endpoint(), &response);
        return Err(anyhow!(
            "get stream uri failed with HTTP {}",
            response.status
        ));
    }
    extract_uri(&response.body)
}

fn extract_uri(body: &str) -> Result<String> {
    let doc = Document::parse(body).map_err(|err| anyhow!("invalid stream uri XML: {err}"))?;
    let uri = doc
        .descendants()
        .find(|node| node.has_tag_name("Uri"))
        .and_then(|node| node.text())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| anyhow!("Stream URI not found"))?;
    Ok(uri)
}
