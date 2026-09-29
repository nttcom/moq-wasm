use crate::{
    app_config::Target,
    onvif_profiles,
    onvif_requests::{self, OnvifRequest},
    onvif_services, ptz_config, soap_client,
};
use anyhow::{anyhow, Result};
use reqwest::Client;

pub struct OnvifClient {
    client: Client,
    target: Target,
    device_endpoint: String,
    media_endpoint: String,
    ptz_endpoint: String,
    profile_token: String,
    ptz_range: ptz_config::PtzRange,
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
        }
    }

    pub async fn initialize(client: Client, target: Target) -> Result<Self> {
        let mut onvif = Self::new(client, target);

        onvif.init_endpoints().await;
        let config_token = onvif.init_profiles().await?;
        onvif.init_ptz(config_token.as_deref()).await;
        Ok(onvif)
    }

    pub async fn initialize_media(client: Client, target: Target) -> Result<Self> {
        let mut onvif = Self::new(client, target);
        onvif.init_endpoints().await;
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

    async fn init_endpoints(&mut self) {
        let endpoints = self.fetch_endpoints().await;
        self.set_endpoints(endpoints);
    }

    async fn init_profiles(&mut self) -> Result<Option<String>> {
        let onvif_profiles::ProfileTokens {
            profile_token,
            config_token,
        } = self.fetch_profile_tokens().await?;
        self.set_profile_token(profile_token);
        Ok(config_token)
    }

    async fn init_ptz(&mut self, token_hint: Option<&str>) {
        match self.fetch_ptz_range(token_hint).await {
            Ok(range) => self.ptz_range = range,
            Err(err) => log::warn!("PTZ error: ptz init error: {err}"),
        }
    }

    async fn fetch_endpoints(&self) -> onvif_services::ServiceEndpoints {
        onvif_services::discover_endpoints(self).await
    }

    async fn fetch_profile_tokens(&self) -> Result<onvif_profiles::ProfileTokens> {
        onvif_profiles::fetch(self).await
    }

    async fn fetch_ptz_range(&self, token_hint: Option<&str>) -> Result<ptz_config::PtzRange> {
        let (token, body) = self.fetch_ptz_config_token(token_hint).await?;
        let range = ptz_config::extract_range_from_config(&body, &token);
        let options_body = self.fetch_ptz_config_options(&token).await?;
        Ok(ptz_config::update_range_from_options(range, &options_body))
    }

    async fn fetch_ptz_config_token(&self, token_hint: Option<&str>) -> Result<(String, String)> {
        log::info!("[GetToken]");
        log::info!("  [GetConfigurations]");
        let cmd = onvif_requests::get_configurations();
        let response = self.send_ptz(&cmd).await?;
        soap_client::log_response_with_prefix(
            "  ",
            "GetConfigurations",
            self.ptz_endpoint(),
            &response,
        );
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
        let cmd = onvif_requests::get_configuration_options(token);
        let response = self.send_ptz(&cmd).await?;
        soap_client::log_response("GetConfigurationOptions", self.ptz_endpoint(), &response);
        if response.status >= 400 {
            return Err(anyhow!(
                "get configuration options failed with HTTP {}",
                response.status
            ));
        }
        Ok(response.body)
    }

    pub async fn send_device(&self, command: &OnvifRequest) -> Result<soap_client::SoapResponse> {
        self.send_to(&self.device_endpoint, command).await
    }

    pub async fn send_media(&self, command: &OnvifRequest) -> Result<soap_client::SoapResponse> {
        self.send_to(&self.media_endpoint, command).await
    }

    pub async fn send_ptz(&self, command: &OnvifRequest) -> Result<soap_client::SoapResponse> {
        self.send_to(&self.ptz_endpoint, command).await
    }

    async fn send_to(
        &self,
        endpoint: &str,
        command: &OnvifRequest,
    ) -> Result<soap_client::SoapResponse> {
        let action = format!("{}/{}", command.namespace, command.operation);
        soap_client::send(
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
