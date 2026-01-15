use crate::cli::OnvifAuth;
use crate::config::Target;
use crate::wsse;
use anyhow::{anyhow, Context, Result};
use reqwest::Client;

pub struct SoapResponse {
    pub status: u16,
    pub body: String,
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
    let mut request = client
        .post(endpoint)
        .header("Content-Type", content_type)
        .body(envelope);
    if matches!(target.onvif_auth(), OnvifAuth::Basic) {
        if let Some((user, pass)) = target.basic_auth() {
            request = request.basic_auth(user, Some(pass));
        }
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("soap request failed for {} -> {}", action, endpoint))?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();

    Ok(SoapResponse { status, body })
}

fn build_envelope(target: &Target, body: &str, namespaces: &str) -> Result<String> {
    let header = match target.onvif_auth() {
        OnvifAuth::Basic => String::new(),
        OnvifAuth::Wsse => {
            let (user, pass) = target
                .basic_auth()
                .ok_or_else(|| anyhow!("onvif wsse auth requires username and password"))?;
            wsse::build_wsse_header(user, pass)?
        }
    };
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
