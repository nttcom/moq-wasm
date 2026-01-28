use crate::config::Target;
use crate::wsse;
use anyhow::{Context, Result};
use reqwest::Client;

pub struct SoapResponse {
    pub status: u16,
    pub body: String,
}

pub fn log_response(action: &str, endpoint: &str, response: &SoapResponse) {
    log_response_with_prefix("", action, endpoint, response);
}

pub fn log_response_with_prefix(
    prefix: &str,
    action: &str,
    endpoint: &str,
    response: &SoapResponse,
) {
    let ok = response.status == 200;
    let status = if ok { "OK" } else { "NG" };
    if ok {
        log::info!(
            "{prefix}SOAP response ({}): {} -> HTTP {} ({})",
            action,
            endpoint,
            response.status,
            status
        );
    } else {
        log::warn!(
            "{prefix}SOAP response ({}): {} -> HTTP {} ({})",
            action,
            endpoint,
            response.status,
            status
        );
    }
}

pub async fn send(
    client: &Client,
    target: &Target,
    endpoint: &str,
    action: &str,
    body: &str,
    namespaces: &str,
) -> Result<SoapResponse> {
    let envelope = build_envelope(target, body, namespaces)?;
    let content_type = "text/xml; charset=utf-8";
    let request = client
        .post(endpoint)
        .header("Content-Type", content_type)
        .body(envelope);
    let response = request
        .send()
        .await
        .with_context(|| format!("soap request failed for {} -> {}", action, endpoint))?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();

    Ok(SoapResponse { status, body })
}

fn build_envelope(target: &Target, body: &str, namespaces: &str) -> Result<String> {
    let (user, pass) = target.credentials();
    let header = wsse::build_wsse_header(user, pass)?;
    Ok(format!(
        r#"<s:Envelope
  xmlns:s="http://www.w3.org/2003/05/soap-envelope"{namespaces}>
{header}  <s:Body xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
    {body}
  </s:Body>
</s:Envelope>
"#
    ))
}
