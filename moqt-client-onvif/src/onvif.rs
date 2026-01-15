use crate::config::Target;
use crate::http_client;
use crate::soap;
use anyhow::{Context, Result};

const SOAP_ACTION: &str = "http://www.onvif.org/ver10/device/wsdl/GetDeviceInformation";
const SOAP_NAMESPACES: &str = r#"
  xmlns:tds="http://www.onvif.org/ver10/device/wsdl""#;
const SOAP_BODY: &str = "<tds:GetDeviceInformation/>";

#[derive(Debug)]
pub struct OnvifProbeResult {
    pub endpoint: String,
    pub http_status: u16,
    pub body_size: usize,
    pub body: String,
    pub has_device_info: bool,
    pub has_soap_fault: bool,
}

pub async fn probe(target: &Target) -> Result<OnvifProbeResult> {
    let client = http_client::build(target).context("onvif client build failed")?;

    let response = soap::send(
        &client,
        target,
        &target.onvif_endpoint(),
        SOAP_ACTION,
        SOAP_BODY,
        SOAP_NAMESPACES,
    )
    .await
    .context("onvif request failed")?;
    let status = response.status;
    let body = response.body;
    let has_device_info = body.contains("GetDeviceInformationResponse");
    let has_soap_fault = body.contains("Fault");

    Ok(OnvifProbeResult {
        endpoint: target.onvif_endpoint(),
        http_status: status,
        body_size: body.len(),
        body,
        has_device_info,
        has_soap_fault,
    })
}
