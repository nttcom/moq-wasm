use crate::{onvif_client::OnvifClient, onvif_requests, ptz_config::find_text};
use roxmltree::Document;
pub(crate) struct ServiceEndpoints {
    pub(crate) media_endpoint: String,
    pub(crate) ptz_endpoint: String,
}
pub(crate) async fn discover_endpoints(onvif: &OnvifClient) -> ServiceEndpoints {
    let (mut media, mut ptz) = match onvif.send_device(&onvif_requests::get_services()).await {
        Ok(body) => parse_services(&body),
        _ => (None, None),
    };
    if (media.is_none() || ptz.is_none())
        && let Ok(body) = onvif.send_device(&onvif_requests::get_capabilities()).await
    {
        let (caps_media, caps_ptz) = parse_capabilities(&body);
        media = media.or(caps_media);
        ptz = ptz.or(caps_ptz);
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
        let namespace = find_text(service, "Namespace");
        let xaddr = find_text(service, "XAddr");
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
        .and_then(|node| find_text(node, "XAddr"));
    let ptz = doc
        .descendants()
        .find(|node| node.has_tag_name("PTZ"))
        .and_then(|node| find_text(node, "XAddr"));
    (media, ptz)
}
