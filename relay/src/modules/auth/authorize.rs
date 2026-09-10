use crate::modules::auth::verified_token::VerifiedToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    Publish,
    Subscribe,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Denied {
    pub(crate) reason: &'static str,
}

pub(crate) fn authorize(
    token: &VerifiedToken,
    operation: Operation,
    namespace: &[String],
) -> Result<(), Denied> {
    let Some((app_id, relative_path)) = namespace.split_first() else {
        return Err(Denied {
            reason: "namespace is empty",
        });
    };
    if namespace.iter().any(|element| element.contains('/')) {
        return Err(Denied {
            reason: "namespace element contains '/'",
        });
    }
    if !token.is_relay && *app_id != token.app_id {
        return Err(Denied {
            reason: "namespace does not belong to the token's appId",
        });
    }
    let granted = match operation {
        Operation::Publish => token.publish.as_deref(),
        Operation::Subscribe => token.subscribe.as_deref(),
    };
    let Some(granted) = granted else {
        return Err(Denied {
            reason: match operation {
                Operation::Publish => "token does not grant publish",
                Operation::Subscribe => "token does not grant subscribe",
            },
        });
    };
    if relative_path.starts_with(granted) {
        Ok(())
    } else {
        Err(Denied {
            reason: "namespace is outside the granted path",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Operation, authorize};
    use crate::modules::auth::verified_token::{VerifiedToken, parse_namespace_path};

    fn app_token(publish: Option<&str>, subscribe: Option<&str>) -> VerifiedToken {
        VerifiedToken {
            app_id: "APP".to_string(),
            publish: publish.map(parse_namespace_path),
            subscribe: subscribe.map(parse_namespace_path),
            is_relay: false,
            expires_at: None,
        }
    }

    fn relay_token() -> VerifiedToken {
        VerifiedToken {
            app_id: "RELAY".to_string(),
            publish: Some(vec![]),
            subscribe: Some(vec![]),
            is_relay: true,
            expires_at: None,
        }
    }

    fn namespace(elements: &[&str]) -> Vec<String> {
        elements.iter().map(|element| element.to_string()).collect()
    }

    #[test]
    fn granted_prefix_allows_equal_and_longer_namespaces() {
        // Arrange
        let token = app_token(Some("site1"), None);

        // Act / Assert
        assert!(authorize(&token, Operation::Publish, &namespace(&["APP", "site1"])).is_ok());
        assert!(
            authorize(
                &token,
                Operation::Publish,
                &namespace(&["APP", "site1", "cam1"])
            )
            .is_ok()
        );
    }

    #[test]
    fn prefix_match_is_per_element_not_per_character() {
        // Arrange
        let token = app_token(Some("site1"), None);

        // Act
        let denied = authorize(&token, Operation::Publish, &namespace(&["APP", "site10"]));

        // Assert
        assert_eq!(
            denied.unwrap_err().reason,
            "namespace is outside the granted path"
        );
    }

    #[test]
    fn shorter_namespace_than_granted_path_is_denied() {
        // Arrange
        let token = app_token(Some("site1"), None);

        // Act
        let denied = authorize(&token, Operation::Publish, &namespace(&["APP"]));

        // Assert
        assert_eq!(
            denied.unwrap_err().reason,
            "namespace is outside the granted path"
        );
    }

    #[test]
    fn sibling_namespace_is_denied() {
        // Arrange
        let token = app_token(Some("site1"), None);

        // Act
        let denied = authorize(&token, Operation::Publish, &namespace(&["APP", "site2"]));

        // Assert
        assert!(denied.is_err());
    }

    #[test]
    fn root_claim_allows_everything_under_the_app_id() {
        // Arrange
        let token = app_token(Some(""), Some(""));

        // Act / Assert
        assert!(authorize(&token, Operation::Publish, &namespace(&["APP"])).is_ok());
        assert!(
            authorize(
                &token,
                Operation::Subscribe,
                &namespace(&["APP", "anything", "deep"])
            )
            .is_ok()
        );
    }

    #[test]
    fn root_claim_does_not_reach_other_app_ids() {
        // Arrange
        let token = app_token(Some(""), Some(""));

        // Act
        let denied = authorize(&token, Operation::Publish, &namespace(&["OTHER", "x"]));

        // Assert
        assert_eq!(
            denied.unwrap_err().reason,
            "namespace does not belong to the token's appId"
        );
    }

    #[test]
    fn namespace_element_containing_slash_is_denied() {
        // Arrange
        let token = app_token(Some(""), None);

        // Act
        let denied = authorize(
            &token,
            Operation::Publish,
            &namespace(&["APP", "site1/cam1"]),
        );

        // Assert
        assert_eq!(denied.unwrap_err().reason, "namespace element contains '/'");
    }

    #[test]
    fn operation_without_matching_claim_is_denied() {
        // Arrange
        let token = app_token(Some("site1"), None);

        // Act
        let denied = authorize(&token, Operation::Subscribe, &namespace(&["APP", "site1"]));

        // Assert
        assert_eq!(denied.unwrap_err().reason, "token does not grant subscribe");
    }

    #[test]
    fn empty_namespace_is_denied() {
        // Arrange
        let token = app_token(Some(""), None);

        // Act
        let denied = authorize(&token, Operation::Publish, &[]);

        // Assert
        assert_eq!(denied.unwrap_err().reason, "namespace is empty");
    }

    #[test]
    fn relay_token_skips_the_app_id_check() {
        // Arrange
        let token = relay_token();

        // Act / Assert
        assert!(
            authorize(
                &token,
                Operation::Subscribe,
                &namespace(&["APP", "site1", "cam1"])
            )
            .is_ok()
        );
        assert!(authorize(&token, Operation::Publish, &namespace(&["OTHER"])).is_ok());
    }

    #[test]
    fn anonymous_token_reaches_only_the_anon_namespace() {
        // Arrange
        let token = VerifiedToken::anonymous();

        // Act / Assert
        assert!(authorize(&token, Operation::Publish, &namespace(&["anon", "room"])).is_ok());
        assert!(
            authorize(
                &token,
                Operation::Subscribe,
                &namespace(&["anon", "room", "cam"])
            )
            .is_ok()
        );
        assert_eq!(
            authorize(&token, Operation::Publish, &namespace(&["APP", "room"]))
                .unwrap_err()
                .reason,
            "namespace does not belong to the token's appId"
        );
    }

    #[test]
    fn relay_token_still_rejects_slash_in_elements() {
        // Arrange
        let token = relay_token();

        // Act
        let denied = authorize(&token, Operation::Publish, &namespace(&["APP", "a/b"]));

        // Assert
        assert!(denied.is_err());
    }
}
