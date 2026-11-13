use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::Claims;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimPolicy {
    /// A relay token is exempt.
    pub max_token_ttl: Duration,
    pub clock_leeway: Duration,
}

impl Default for ClaimPolicy {
    fn default() -> Self {
        Self {
            max_token_ttl: Duration::from_secs(24 * 60 * 60),
            clock_leeway: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ClaimError {
    #[error("token has no iat or exp")]
    MissingLifetime,
    #[error("expired")]
    Expired,
    #[error("not_yet_valid")]
    NotYetValid,
    #[error("ttl_too_long")]
    TtlTooLong,
    #[error("namespace path contains an empty element")]
    EmptyElement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Publish,
    Subscribe,
}

/// A token the VTS `POST /verify` found signed by its app's secret. The VTS
/// checks only the signature: `check` is the caller's.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedToken {
    pub app_id: String,
    pub is_relay: bool,
    pub claims: Claims,
}

impl SignedToken {
    /// Checks the lifetime and that each namespace claim is `""` or
    /// `/`-separated non-empty elements.
    pub fn check(&self, policy: ClaimPolicy, now: SystemTime) -> Result<(), ClaimError> {
        let (Some(iat), Some(exp)) = (self.claims.iat, self.claims.exp) else {
            return Err(ClaimError::MissingLifetime);
        };
        if SystemTime::UNIX_EPOCH + Duration::from_secs(exp) + policy.clock_leeway <= now {
            return Err(ClaimError::Expired);
        }
        if SystemTime::UNIX_EPOCH + Duration::from_secs(iat) > now + policy.clock_leeway {
            return Err(ClaimError::NotYetValid);
        }
        if !self.is_relay && exp.saturating_sub(iat) > policy.max_token_ttl.as_secs() {
            return Err(ClaimError::TtlTooLong);
        }
        for claim in [&self.claims.publish, &self.claims.subscribe]
            .into_iter()
            .flatten()
        {
            if !claim.is_empty() && claim.split('/').any(str::is_empty) {
                return Err(ClaimError::EmptyElement);
            }
        }
        Ok(())
    }

    /// Whether the claim for `access` covers `namespace`, a full namespace
    /// starting with the app id, element by element: `""` covers the whole
    /// app, `room` covers `<appId>/room` and below but not `<appId>/room2`.
    pub fn covers(&self, access: Access, namespace: &str) -> bool {
        let claim = match access {
            Access::Publish => &self.claims.publish,
            Access::Subscribe => &self.claims.subscribe,
        };
        let Some(claim) = claim else {
            return false;
        };
        let allowed = if claim.is_empty() {
            self.app_id.clone()
        } else {
            format!("{}/{claim}", self.app_id)
        };
        namespace == allowed
            || namespace
                .strip_prefix(&allowed)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    const NOW_SECS: u64 = 1_759_600_000;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(NOW_SECS)
    }

    fn signed(is_relay: bool, iat: u64, exp: u64, publish: Option<&str>) -> SignedToken {
        SignedToken {
            app_id: "APP".to_string(),
            is_relay,
            claims: Claims {
                app_id: "APP".to_string(),
                publish: publish.map(str::to_string),
                subscribe: Some("site1".to_string()),
                iat: Some(iat),
                exp: Some(exp),
                relay_id: None,
            },
        }
    }

    fn check(token: &SignedToken) -> Result<(), ClaimError> {
        token.check(ClaimPolicy::default(), now())
    }

    #[test]
    fn a_vts_answer_deserializes() {
        // Act
        let token: SignedToken = serde_json::from_str(
            r#"{"appId":"APP","isRelay":true,"claims":{"appId":"APP","publish":"","iat":1,"exp":2}}"#,
        )
        .unwrap();

        // Assert
        assert_eq!(token.app_id, "APP");
        assert!(token.is_relay);
        assert_eq!(token.claims.publish.as_deref(), Some(""));
    }

    #[test]
    fn a_valid_client_token_passes() {
        // Act / Assert
        assert_eq!(
            check(&signed(
                false,
                NOW_SECS - 10,
                NOW_SECS + 3600,
                Some("site1/cam1")
            )),
            Ok(())
        );
    }

    #[test]
    fn missing_exp_is_rejected() {
        // Arrange
        let mut token = signed(false, NOW_SECS, NOW_SECS + 60, None);
        token.claims.exp = None;

        // Act / Assert
        assert_eq!(check(&token), Err(ClaimError::MissingLifetime));
    }

    #[test]
    fn expiry_is_judged_with_the_leeway() {
        // Act / Assert
        assert_eq!(
            check(&signed(false, NOW_SECS - 200, NOW_SECS - 61, None)),
            Err(ClaimError::Expired)
        );
        assert_eq!(
            check(&signed(false, NOW_SECS - 200, NOW_SECS - 30, None)),
            Ok(())
        );
    }

    #[test]
    fn issued_in_the_future_beyond_the_leeway_is_rejected() {
        // Act / Assert
        assert_eq!(
            check(&signed(false, NOW_SECS + 120, NOW_SECS + 3600, None)),
            Err(ClaimError::NotYetValid)
        );
    }

    #[test]
    fn only_a_relay_token_may_outlive_the_maximum_ttl() {
        // Act / Assert
        assert_eq!(
            check(&signed(false, NOW_SECS, NOW_SECS + 24 * 3600 + 1, None)),
            Err(ClaimError::TtlTooLong)
        );
        assert_eq!(
            check(&signed(
                true,
                NOW_SECS,
                NOW_SECS + 365 * 24 * 3600,
                Some("")
            )),
            Ok(())
        );
    }

    #[test]
    fn a_path_with_an_empty_element_is_rejected() {
        // Act / Assert
        assert_eq!(
            check(&signed(false, NOW_SECS, NOW_SECS + 60, Some("site1//cam1"))),
            Err(ClaimError::EmptyElement)
        );
    }

    #[test]
    fn claims_cover_namespaces_on_element_boundaries() {
        // Arrange
        let token = signed(false, NOW_SECS, NOW_SECS + 60, None);

        // Act / Assert
        assert!(token.covers(Access::Subscribe, "APP/site1"));
        assert!(token.covers(Access::Subscribe, "APP/site1/recordings/id"));
        assert!(!token.covers(Access::Subscribe, "APP/site10"));
        assert!(!token.covers(Access::Subscribe, "APP"));
        assert!(!token.covers(Access::Subscribe, "OTHER/site1"));
        assert!(!token.covers(Access::Publish, "APP/site1"));
    }

    #[test]
    fn an_empty_claim_covers_the_whole_app() {
        // Arrange
        let token = signed(false, NOW_SECS, NOW_SECS + 60, Some(""));

        // Act / Assert
        assert!(token.covers(Access::Publish, "APP"));
        assert!(token.covers(Access::Publish, "APP/any/room"));
        assert!(!token.covers(Access::Publish, "APPX"));
    }
}
