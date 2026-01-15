use crate::config::Target;
use crate::soap;
use anyhow::{Context, Result};
use reqwest::Client;

pub async fn send_with_device_fallback(
    client: &Client,
    target: &Target,
    endpoint: &str,
    action: &str,
    body: &str,
    namespaces: &str,
    log_responses: bool,
) -> Result<soap::SoapResponse> {
    match soap::send(client, target, endpoint, action, body, namespaces).await {
        Ok(response) => {
            log_response(action, endpoint, &response, log_responses);
            Ok(response)
        }
        Err(err) => {
            if should_fallback(endpoint, target, &err) {
                let fallback = target.onvif_endpoint();
                eprintln!("PTZ retry via device service: {}", fallback);
                let response = soap::send(client, target, &fallback, action, body, namespaces)
                    .await
                    .with_context(|| {
                        format!("ptz device service retry failed for {} -> {}", action, fallback)
                    })?;
                log_response(action, &fallback, &response, log_responses);
                return Ok(response);
            }
            Err(err)
        }
    }
}

fn should_fallback(endpoint: &str, target: &Target, err: &anyhow::Error) -> bool {
    endpoint != target.onvif_endpoint() && has_transport_error(err)
}

fn has_transport_error(err: &anyhow::Error) -> bool {
    err.chain().any(|source| {
        let msg = source.to_string();
        msg.contains("invalid HTTP version parsed") || msg.contains("tls handshake eof")
    })
}

fn log_response(
    action: &str,
    endpoint: &str,
    response: &soap::SoapResponse,
    enabled: bool,
) {
    if !enabled {
        return;
    }
    if response.body.is_empty() {
        println!(
            "SOAP response ({}): {} -> HTTP {} (empty body)",
            action, endpoint, response.status
        );
    } else {
        println!(
            "SOAP response ({}): {} -> HTTP {}\n{}",
            action, endpoint, response.status, response.body
        );
    }
}
