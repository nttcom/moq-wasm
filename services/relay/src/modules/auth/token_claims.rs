use std::time::{Duration, SystemTime};

use auth_token::{ClaimError, ClaimPolicy, SignedToken};

use crate::modules::auth::verified_token::{VerifiedToken, parse_namespace_path};

pub(crate) fn build_verified_token(
    token: SignedToken,
    policy: ClaimPolicy,
    now: SystemTime,
) -> Result<VerifiedToken, ClaimError> {
    token.check(policy, now)?;
    let expires_at = token
        .claims
        .exp
        .map(|exp| SystemTime::UNIX_EPOCH + Duration::from_secs(exp));
    Ok(VerifiedToken {
        app_id: token.app_id,
        publish: token.claims.publish.as_deref().map(parse_namespace_path),
        subscribe: token.claims.subscribe.as_deref().map(parse_namespace_path),
        is_relay: token.is_relay,
        expires_at,
        relay_id: token.is_relay.then_some(token.claims.relay_id).flatten(),
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use auth_token::{ClaimError, ClaimPolicy, Claims, SignedToken};

    use super::build_verified_token;

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
    fn a_relay_token_names_its_relay() {
        // Arrange
        let mut token = signed(true, NOW_SECS, NOW_SECS + 60, Some(""));
        token.claims.relay_id = Some("relay-b".to_string());

        // Act
        let verified = build_verified_token(token, ClaimPolicy::default(), now()).unwrap();

        // Assert
        assert_eq!(verified.relay_id.as_deref(), Some("relay-b"));
    }

    #[test]
    fn a_client_token_naming_a_relay_is_not_taken_for_one() {
        // Arrange
        let mut token = signed(false, NOW_SECS, NOW_SECS + 60, Some(""));
        token.claims.relay_id = Some("relay-b".to_string());

        // Act
        let verified = build_verified_token(token, ClaimPolicy::default(), now()).unwrap();

        // Assert
        assert_eq!(verified.relay_id, None);
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
    fn a_token_failing_the_claim_checks_is_rejected() {
        // Act / Assert
        assert_eq!(
            build_verified_token(
                signed(false, NOW_SECS - 200, NOW_SECS - 61, None),
                ClaimPolicy::default(),
                now()
            ),
            Err(ClaimError::Expired)
        );
    }
}
