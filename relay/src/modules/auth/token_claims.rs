use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::modules::auth::verified_token::{VerifiedToken, parse_namespace_path};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TokenClaims {
    pub(crate) publish: Option<String>,
    pub(crate) subscribe: Option<String>,
    pub(crate) iat: Option<u64>,
    pub(crate) exp: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimPolicy {
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

pub(crate) struct SignedToken {
    pub(crate) app_id: String,
    pub(crate) is_relay: bool,
    pub(crate) claims: TokenClaims,
}

pub(crate) fn build_verified_token(
    token: SignedToken,
    policy: ClaimPolicy,
    now: SystemTime,
) -> Result<VerifiedToken, &'static str> {
    let (Some(iat), Some(exp)) = (token.claims.iat, token.claims.exp) else {
        return Err("token has no iat or exp");
    };
    let issued_at = SystemTime::UNIX_EPOCH + Duration::from_secs(iat);
    let expires_at = SystemTime::UNIX_EPOCH + Duration::from_secs(exp);
    if expires_at + policy.clock_leeway <= now {
        return Err("expired");
    }
    if issued_at > now + policy.clock_leeway {
        return Err("not_yet_valid");
    }
    if !token.is_relay && exp.saturating_sub(iat) > policy.max_token_ttl.as_secs() {
        return Err("ttl_too_long");
    }
    let publish = namespace_path_claim(token.claims.publish.as_deref())?;
    let subscribe = namespace_path_claim(token.claims.subscribe.as_deref())?;
    Ok(VerifiedToken {
        app_id: token.app_id,
        publish,
        subscribe,
        is_relay: token.is_relay,
        expires_at: Some(expires_at),
    })
}

fn namespace_path_claim(claim: Option<&str>) -> Result<Option<Vec<String>>, &'static str> {
    let Some(claim) = claim else {
        return Ok(None);
    };
    if !claim.is_empty() && claim.split('/').any(|element| element.is_empty()) {
        return Err("namespace path contains an empty element");
    }
    Ok(Some(parse_namespace_path(claim)))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{ClaimPolicy, SignedToken, TokenClaims, build_verified_token};

    const NOW_SECS: u64 = 1_759_600_000;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(NOW_SECS)
    }

    fn signed(is_relay: bool, iat: u64, exp: u64, publish: Option<&str>) -> SignedToken {
        SignedToken {
            app_id: "APP".to_string(),
            is_relay,
            claims: TokenClaims {
                publish: publish.map(str::to_string),
                subscribe: Some("site1".to_string()),
                iat: Some(iat),
                exp: Some(exp),
            },
        }
    }

    #[test]
    fn valid_client_token_becomes_a_verified_token() {
        // Act
        let token = build_verified_token(
            signed(false, NOW_SECS - 10, NOW_SECS + 3600, Some("site1/cam1")),
            ClaimPolicy::default(),
            now(),
        )
        .unwrap();

        // Assert
        assert_eq!(token.app_id, "APP");
        assert_eq!(
            token.publish,
            Some(vec!["site1".to_string(), "cam1".to_string()])
        );
        assert_eq!(token.subscribe, Some(vec!["site1".to_string()]));
        assert_eq!(
            token.expires_at,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(NOW_SECS + 3600))
        );
    }

    #[test]
    fn empty_claim_is_the_app_root_and_absent_claim_stays_absent() {
        // Arrange
        let mut token = signed(false, NOW_SECS, NOW_SECS + 60, Some(""));
        token.claims.subscribe = None;

        // Act
        let verified = build_verified_token(token, ClaimPolicy::default(), now()).unwrap();

        // Assert
        assert_eq!(verified.publish, Some(vec![]));
        assert_eq!(verified.subscribe, None);
    }

    #[test]
    fn missing_exp_is_rejected() {
        // Arrange
        let mut token = signed(false, NOW_SECS, NOW_SECS + 60, None);
        token.claims.exp = None;

        // Act / Assert
        assert_eq!(
            build_verified_token(token, ClaimPolicy::default(), now()),
            Err("token has no iat or exp")
        );
    }

    #[test]
    fn expired_beyond_the_leeway_is_rejected() {
        // Act / Assert
        assert_eq!(
            build_verified_token(
                signed(false, NOW_SECS - 200, NOW_SECS - 61, None),
                ClaimPolicy::default(),
                now()
            ),
            Err("expired")
        );
    }

    #[test]
    fn expired_within_the_leeway_is_accepted() {
        // Act / Assert
        assert!(
            build_verified_token(
                signed(false, NOW_SECS - 200, NOW_SECS - 30, None),
                ClaimPolicy::default(),
                now()
            )
            .is_ok()
        );
    }

    #[test]
    fn issued_in_the_future_beyond_the_leeway_is_rejected() {
        // Act / Assert
        assert_eq!(
            build_verified_token(
                signed(false, NOW_SECS + 120, NOW_SECS + 3600, None),
                ClaimPolicy::default(),
                now()
            ),
            Err("not_yet_valid")
        );
    }

    #[test]
    fn client_token_longer_than_the_maximum_ttl_is_rejected() {
        // Act / Assert
        assert_eq!(
            build_verified_token(
                signed(false, NOW_SECS, NOW_SECS + 24 * 3600 + 1, None),
                ClaimPolicy::default(),
                now()
            ),
            Err("ttl_too_long")
        );
    }

    #[test]
    fn relay_token_may_outlive_the_maximum_ttl() {
        // Act / Assert
        assert!(
            build_verified_token(
                signed(true, NOW_SECS, NOW_SECS + 365 * 24 * 3600, Some("")),
                ClaimPolicy::default(),
                now()
            )
            .is_ok()
        );
    }

    #[test]
    fn path_with_an_empty_element_is_rejected() {
        // Act / Assert
        assert_eq!(
            build_verified_token(
                signed(false, NOW_SECS, NOW_SECS + 60, Some("site1//cam1")),
                ClaimPolicy::default(),
                now()
            ),
            Err("namespace path contains an empty element")
        );
    }
}
