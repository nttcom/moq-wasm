use anyhow::{anyhow, Context, Result};
use roxmltree::Document;

pub struct ServiceEndpoint {
    pub namespace: String,
    pub xaddr: String,
}

pub struct ServiceEndpoints {
    pub media: ServiceEndpoint,
    pub ptz: ServiceEndpoint,
}

pub fn parse_service(body: &str, needle: &str) -> Option<ServiceEndpoint> {
    let doc = Document::parse(body).ok()?;
    for service in doc
        .descendants()
        .filter(|node| node.tag_name().name() == "Service")
    {
        let namespace = service
            .descendants()
            .find(|node| node.tag_name().name() == "Namespace")?
            .text()?;
        if !namespace.contains(needle) {
            continue;
        }
        let xaddr_text = service
            .descendants()
            .find(|node| node.tag_name().name() == "XAddr")?
            .text()?;
        let xaddr = select_xaddr(xaddr_text)?;
        return Some(ServiceEndpoint {
            namespace: namespace.to_string(),
            xaddr,
        });
    }
    None
}

pub fn parse_profile_token(body: &str) -> Result<String> {
    let doc = Document::parse(body).context("parse get profiles response failed")?;
    let token = doc
        .descendants()
        .find(|node| node.tag_name().name() == "Profiles")
        .and_then(|node| node.attribute("token"))
        .ok_or_else(|| anyhow!("profile token not found in response"))?;
    Ok(token.to_string())
}

fn select_xaddr(text: &str) -> Option<String> {
    let candidates: Vec<&str> = text.split_whitespace().filter(|s| !s.is_empty()).collect();
    if candidates.is_empty() {
        return None;
    }
    let preferred = candidates
        .iter()
        .find(|addr| addr.starts_with("http://"))
        .or_else(|| candidates.iter().find(|addr| addr.starts_with("https://")))
        .or_else(|| candidates.first())
        .copied()?;
    Some(preferred.to_string())
}
