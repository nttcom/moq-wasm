use anyhow::{Context, bail};
use hyper::HeaderMap;
use reqwest::Url;

use crate::iap::IapVerifier;

pub const IAP_ASSERTION_HEADER: &str = "x-goog-iap-jwt-assertion";

pub enum Identity {
    Iap(Box<IapVerifier>),
    Development { sub: String },
}

pub struct IdentitySettings {
    pub iap_audience: Option<String>,
    pub development_sub: Option<String>,
    pub on_cloud_run: bool,
}

impl Identity {
    /// Fails when the settings name no identity, both identities, or a development identity on
    /// Cloud Run.
    pub fn new(settings: IdentitySettings, iap_keys_url: Url) -> anyhow::Result<Self> {
        match (settings.iap_audience, settings.development_sub) {
            (Some(_), Some(_)) => bail!("set CONSOLE_IAP_AUDIENCE or CONSOLE_DEV_SUB, not both"),
            (None, Some(_)) if settings.on_cloud_run => {
                bail!("CONSOLE_DEV_SUB must not be set on Cloud Run")
            }
            (None, Some(sub)) => Ok(Self::Development { sub }),
            (Some(audience), None) => Ok(Self::Iap(Box::new(IapVerifier::new(
                audience,
                iap_keys_url,
            )))),
            (None, None) => bail!("set CONSOLE_IAP_AUDIENCE"),
        }
    }

    pub async fn sub(&self, headers: &HeaderMap, now: u64) -> anyhow::Result<String> {
        match self {
            Self::Development { sub } => Ok(sub.clone()),
            Self::Iap(verifier) => {
                let assertion = headers
                    .get(IAP_ASSERTION_HEADER)
                    .context("the request has no IAP assertion")?
                    .to_str()
                    .context("the IAP assertion is not ASCII")?;
                verifier.verify(assertion, now).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{AUDIENCE, NOW};

    fn settings(
        iap_audience: Option<&str>,
        development_sub: Option<&str>,
        on_cloud_run: bool,
    ) -> IdentitySettings {
        IdentitySettings {
            iap_audience: iap_audience.map(Into::into),
            development_sub: development_sub.map(Into::into),
            on_cloud_run,
        }
    }

    fn keys_url() -> Url {
        Url::parse("http://127.0.0.1:9/keys").unwrap()
    }

    #[test]
    fn refuses_a_development_identity_on_cloud_run() {
        // Act / Assert
        assert!(Identity::new(settings(None, Some("dev"), true), keys_url()).is_err());
    }

    #[test]
    fn refuses_both_identities() {
        // Act / Assert
        assert!(Identity::new(settings(Some(AUDIENCE), Some("dev"), false), keys_url()).is_err());
    }

    #[test]
    fn refuses_no_identity() {
        // Act / Assert
        assert!(Identity::new(settings(None, None, false), keys_url()).is_err());
    }

    #[tokio::test]
    async fn development_identity_needs_no_header() {
        // Arrange
        let identity = Identity::new(settings(None, Some("dev"), false), keys_url()).unwrap();

        // Act / Assert
        assert_eq!(identity.sub(&HeaderMap::new(), NOW).await.unwrap(), "dev");
    }

    #[tokio::test]
    async fn iap_identity_rejects_a_request_without_the_assertion() {
        // Arrange
        let identity = Identity::new(settings(Some(AUDIENCE), None, true), keys_url()).unwrap();

        // Act / Assert
        assert!(identity.sub(&HeaderMap::new(), NOW).await.is_err());
    }
}
