use crate::config::Target;
use crate::ptz_defs::{media_namespaces, DEVICE_NAMESPACES, DEVICE_NS, MEDIA_NS, PTZ_NS};
use crate::ptz_parse::{parse_profile_token, parse_service, ServiceEndpoint, ServiceEndpoints};
use crate::ptz_request;
use anyhow::{anyhow, Context, Result};
use reqwest::Client;
pub async fn discover_services(
    client: &Client,
    target: &Target,
    log_responses: bool,
) -> Result<ServiceEndpoints> {
    let action = format!("{}/GetServices", DEVICE_NS);
    let body = "<tds:GetServices><tds:IncludeCapability>false</tds:IncludeCapability></tds:GetServices>";
    let response = ptz_request::send_with_device_fallback(
        client,
        target,
        &target.onvif_endpoint(),
        &action,
        body,
        DEVICE_NAMESPACES,
        log_responses,
    )
    .await
    .context("get services failed")?;
    if response.status >= 400 {
        return Err(anyhow!("get services failed with HTTP {}", response.status));
    }

    let media = parse_service(&response.body, "media/wsdl").unwrap_or(ServiceEndpoint {
        namespace: MEDIA_NS.to_string(),
        xaddr: target.onvif_endpoint(),
    });
    let ptz = parse_service(&response.body, "ptz/wsdl").unwrap_or(ServiceEndpoint {
        namespace: PTZ_NS.to_string(),
        xaddr: target.onvif_endpoint(),
    });
    Ok(ServiceEndpoints { media, ptz })
}
pub async fn get_profile_token(
    client: &Client,
    target: &Target,
    media: &ServiceEndpoint,
    log_responses: bool,
) -> Result<String> {
    let action = format!("{}/GetProfiles", media.namespace);
    let body = "<trt:GetProfiles/>";
    let namespaces = media_namespaces(&media.namespace);
    let response = ptz_request::send_with_device_fallback(
        client,
        target,
        &media.xaddr,
        &action,
        body,
        &namespaces,
        log_responses,
    )
    .await
    .context("get profiles failed")?;
    if response.status >= 400 {
        return Err(anyhow!("get profiles failed with HTTP {}", response.status));
    }
    parse_profile_token(&response.body)
}
