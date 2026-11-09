use crate::{onvif_client::OnvifClient, onvif_requests};
use roxmltree::Document;
pub struct ServiceEndpoints {
    pub media_endpoint: String,
    pub ptz_endpoint: String,
}
pub async fn discover_endpoints(onvif: &OnvifClient) -> ServiceEndpoints {
    let (mut media, mut ptz) = match onvif.send_device(&onvif_requests::get_services()).await {
        Ok(services) if services.status < 400 => parse_services(&services.body),
        _ => (None, None),
    };
    if media.is_none() || ptz.is_none() {
        if let Ok(caps) = onvif.send_device(&onvif_requests::get_capabilities()).await {
            if caps.status < 400 {
                let (caps_media, caps_ptz) = parse_capabilities(&caps.body);
                media = media.or(caps_media);
                ptz = ptz.or(caps_ptz);
            }
        }
    }
    let fallback = onvif.device_endpoint().to_string();
    ServiceEndpoints {
        media_endpoint: media.unwrap_or_else(|| fallback.clone()),
        ptz_endpoint: ptz.unwrap_or(fallback),
    }
}
fn parse_services(body: &str) -> (Option<String>, Option<String>) {
    let Ok(doc) = Document::parse(body) else {
        return (None, None);
    };
    let mut media = None;
    let mut ptz = None;
    for service in doc
        .descendants()
        .filter(|node| node.has_tag_name("Service"))
    {
        let namespace = child_text(service, "Namespace");
        let xaddr = child_text(service, "XAddr");
        if let (Some(ns), Some(addr)) = (namespace, xaddr) {
            if ns.contains("media/wsdl") {
                media = Some(addr.clone());
            }
            if ns.contains("ptz/wsdl") {
                ptz = Some(addr);
            }
        }
    }
    (media, ptz)
}
fn parse_capabilities(body: &str) -> (Option<String>, Option<String>) {
    let Ok(doc) = Document::parse(body) else {
        return (None, None);
    };
    let media = doc
        .descendants()
        .find(|node| node.has_tag_name("Media"))
        .and_then(|node| child_text(node, "XAddr"));
    let ptz = doc
        .descendants()
        .find(|node| node.has_tag_name("PTZ"))
        .and_then(|node| child_text(node, "XAddr"));
    (media, ptz)
}
fn child_text(node: roxmltree::Node, name: &str) -> Option<String> {
    node.children()
        .find(|child| child.is_element() && child.has_tag_name(name))
        .and_then(|child| child.text())
        .map(|text| text.trim().to_string())
}
