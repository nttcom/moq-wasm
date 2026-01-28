use crate::{
    config::Target,
    onvif_command::{self, OnvifCommand},
    onvif_nodes, onvif_profiles, onvif_services, ptz_config, soap,
};
use anyhow::{anyhow, Result};
use reqwest::Client;
use std::fmt;

pub struct OnvifClient {
    client: Client,
    target: Target,
    device_endpoint: String,
    media_endpoint: String,
    ptz_endpoint: String,
    profile_token: String,
    ptz_range: ptz_config::PtzRange,
    ptz_node: Option<onvif_nodes::PtzNodeInfo>,
    gui_messages: Vec<String>,
}

struct PtzRangeInfo {
    config_token: String,
    range: ptz_config::PtzRange,
}

impl OnvifClient {
    fn new(client: Client, target: Target) -> Self {
        let device_endpoint = target.onvif_endpoint();
        Self {
            client,
            target,
            media_endpoint: device_endpoint.clone(),
            ptz_endpoint: device_endpoint.clone(),
            device_endpoint,
            profile_token: String::new(),
            ptz_range: ptz_config::PtzRange::default(),
            ptz_node: None,
            gui_messages: Vec::new(),
        }
    }

    pub async fn initialize(client: Client, target: Target) -> Result<Self> {
        let mut onvif = Self::new(client, target);

        let endpoints = onvif.fetch_endpoints().await;
        onvif.set_endpoints(endpoints);

        let onvif_profiles::ProfileTokens {
            profile_token,
            config_token,
        } = onvif.fetch_profile_tokens().await?;
        onvif.set_profile_token(profile_token);

        let ptz = match onvif.fetch_ptz_range(config_token.as_deref()).await {
            Ok(ptz) => ptz,
            Err(err) => {
                onvif.push_ptz_init_error(err);
                return Ok(onvif);
            }
        };
        onvif.set_ptz_range(ptz.range);
        let node = match onvif.fetch_ptz_node_capabilities(&ptz.config_token).await {
            Ok(node) => node,
            Err(err) => {
                onvif.push_ptz_init_error(err);
                None
            }
        };
        onvif.set_ptz_node(node);
        Ok(onvif)
    }

    pub fn set_endpoints(&mut self, endpoints: onvif_services::ServiceEndpoints) {
        self.media_endpoint = endpoints.media_endpoint;
        self.ptz_endpoint = endpoints.ptz_endpoint;
    }

    pub fn set_profile_token(&mut self, token: String) {
        self.profile_token = token;
    }

    pub fn device_endpoint(&self) -> &str {
        &self.device_endpoint
    }

    pub fn media_endpoint(&self) -> &str {
        &self.media_endpoint
    }

    pub fn ptz_endpoint(&self) -> &str {
        &self.ptz_endpoint
    }

    pub fn profile_token(&self) -> &str {
        &self.profile_token
    }

    pub fn ptz_range(&self) -> ptz_config::PtzRange {
        self.ptz_range.clone()
    }

    pub fn ptz_node(&self) -> Option<onvif_nodes::PtzNodeInfo> {
        self.ptz_node.clone()
    }

    pub fn take_gui_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.gui_messages)
    }

    async fn fetch_endpoints(&self) -> onvif_services::ServiceEndpoints {
        onvif_services::discover_endpoints(self).await
    }

    async fn fetch_profile_tokens(&self) -> Result<onvif_profiles::ProfileTokens> {
        onvif_profiles::fetch(self).await
    }

    async fn fetch_ptz_range(&self, token_hint: Option<&str>) -> Result<PtzRangeInfo> {
        let (token, body) = self.fetch_ptz_config_token(token_hint).await?;
        let range = ptz_config::extract_range_from_config(&body, &token);
        let options_body = self.fetch_ptz_config_options(&token).await?;
        let range = ptz_config::update_range_from_options(range, &options_body);
        Ok(PtzRangeInfo {
            config_token: token,
            range,
        })
    }

    async fn fetch_ptz_config_token(&self, token_hint: Option<&str>) -> Result<(String, String)> {
        log::info!("[GetToken]");
        log::info!("  [GetConfigurations]");
        let cmd = onvif_command::get_configurations();
        let response = self.send_ptz(&cmd).await?;
        soap::log_response_with_prefix("  ", "GetConfigurations", self.ptz_endpoint(), &response);
        if response.status >= 400 {
            return Err(anyhow!(
                "get configurations failed with HTTP {}",
                response.status
            ));
        }
        let body = response.body;
        let tokens = ptz_config::extract_tokens(&body);
        let token = select_ptz_config_token(&tokens, token_hint)
            .ok_or_else(|| anyhow!("PTZ configuration token not found in response"))?;
        Ok((token, body))
    }

    async fn fetch_ptz_config_options(&self, token: &str) -> Result<String> {
        log::info!("[GetConfigurationOptions]");
        let cmd = onvif_command::get_configuration_options(token);
        let response = self.send_ptz(&cmd).await?;
        soap::log_response("GetConfigurationOptions", self.ptz_endpoint(), &response);
        if response.status >= 400 {
            return Err(anyhow!(
                "get configuration options failed with HTTP {}",
                response.status
            ));
        }
        Ok(response.body)
    }

    fn set_ptz_range(&mut self, range: ptz_config::PtzRange) {
        self.ptz_range = range;
    }

    fn set_ptz_node(&mut self, node: Option<onvif_nodes::PtzNodeInfo>) {
        self.ptz_node = node;
    }

    fn push_ptz_init_error(&mut self, err: impl fmt::Display) {
        self.gui_messages.push(format!("ptz init error: {err}"));
    }

    async fn fetch_ptz_node_capabilities(
        &self,
        token: &str,
    ) -> Result<Option<onvif_nodes::PtzNodeInfo>> {
        log::info!("[GetConfiguration]");
        let cmd = onvif_command::get_configuration(token);
        let response = self.send_ptz(&cmd).await?;
        soap::log_response("GetConfiguration", self.ptz_endpoint(), &response);
        if response.status >= 400 {
            if is_no_entity_fault(&response.body) {
                return onvif_nodes::log_nodes(self).await;
            }
            return Err(anyhow!(
                "get configuration failed with HTTP {}",
                response.status
            ));
        }
        match onvif_nodes::log_nodes(self).await {
            Ok(node) => Ok(node),
            Err(err) => {
                log::warn!("PTZ node query failed: {err}");
                Ok(None)
            }
        }
    }

    pub async fn send_device(&self, command: &OnvifCommand) -> Result<soap::SoapResponse> {
        self.send_to(&self.device_endpoint, command).await
    }

    pub async fn send_media(&self, command: &OnvifCommand) -> Result<soap::SoapResponse> {
        self.send_to(&self.media_endpoint, command).await
    }

    pub async fn send_ptz(&self, command: &OnvifCommand) -> Result<soap::SoapResponse> {
        self.send_to(&self.ptz_endpoint, command).await
    }

    async fn send_to(&self, endpoint: &str, command: &OnvifCommand) -> Result<soap::SoapResponse> {
        let action = format!("{}/{}", command.namespace, command.operation);
        soap::send(
            &self.client,
            &self.target,
            endpoint,
            &action,
            &command.body,
            "",
        )
        .await
    }
}

fn select_ptz_config_token(tokens: &[String], hint: Option<&str>) -> Option<String> {
    if tokens.is_empty() {
        return hint.map(str::to_string);
    }
    hint.and_then(|hint| tokens.iter().find(|token| token.as_str() == hint).cloned())
        .or_else(|| tokens.first().cloned())
}

fn is_no_entity_fault(body: &str) -> bool {
    body.contains("No such PTZNode") || body.contains("ter:NoEntity")
}
