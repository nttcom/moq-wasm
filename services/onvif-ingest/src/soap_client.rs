use crate::app_config::Target;
use crate::onvif_requests::OnvifRequest;
use crate::wsse_auth;
use anyhow::{bail, Context, Result};
use reqwest::Client;

pub(crate) fn build(target: &Target) -> Result<Client> {
    Client::builder()
        .timeout(target.timeout())
        .build()
        .context("http client build failed")
}

/// Returns the response body; an HTTP status of 400 or above is logged and returned
/// as an error.
pub(crate) async fn send(
    client: &Client,
    target: &Target,
    endpoint: &str,
    command: &OnvifRequest,
) -> Result<String> {
    let action = format!("{}/{}", command.namespace, command.operation);
    let operation = command.operation;
    let envelope = build_envelope(target, &command.body)?;
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
    if status >= 400 {
        tracing::warn!("SOAP response ({operation}): {endpoint} -> HTTP {status} (NG)");
        bail!("{operation} failed with HTTP {status}");
    }
    Ok(body)
}

fn build_envelope(target: &Target, body: &str) -> Result<String> {
    let (user, pass) = target.credentials();
    let header = wsse_auth::build_wsse_header(user, pass)?;
    Ok(format!(
        r#"<s:Envelope
  xmlns:s="http://www.w3.org/2003/05/soap-envelope">
{header}  <s:Body xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
    {body}
  </s:Body>
</s:Envelope>
"#
    ))
}
