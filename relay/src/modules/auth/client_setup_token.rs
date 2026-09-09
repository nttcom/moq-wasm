use moqt::wire::{AuthorizationToken, ClientSetup};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SetupTokenError {
    #[error("CLIENT_SETUP carries no AUTHORIZATION TOKEN")]
    Missing,
    #[error("AUTHORIZATION TOKEN alias type is not USE_VALUE")]
    UnsupportedAliasType,
    #[error("AUTHORIZATION TOKEN type {0} is not supported (expected 0)")]
    UnsupportedTokenType(u64),
    #[error("AUTHORIZATION TOKEN value is not UTF-8")]
    NotUtf8,
}

pub(crate) fn extract_token(client_setup: &ClientSetup) -> Result<String, SetupTokenError> {
    let token = client_setup
        .setup_parameters
        .authorization_token
        .first()
        .ok_or(SetupTokenError::Missing)?;
    match token {
        AuthorizationToken::UseValue {
            token_type: 0,
            token_value,
        } => String::from_utf8(token_value.to_vec()).map_err(|_| SetupTokenError::NotUtf8),
        AuthorizationToken::UseValue { token_type, .. } => {
            Err(SetupTokenError::UnsupportedTokenType(*token_type))
        }
        AuthorizationToken::Delete
        | AuthorizationToken::Register { .. }
        | AuthorizationToken::UseAlias { .. } => Err(SetupTokenError::UnsupportedAliasType),
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use moqt::wire::AuthorizationToken;

    use super::{SetupTokenError, extract_token};
    use crate::modules::auth::test_support::client_setup;

    #[test]
    fn use_value_token_of_type_zero_is_returned_as_string() {
        // Arrange
        let setup = client_setup(vec![AuthorizationToken::use_value_utf8("jwt")]);

        // Act / Assert
        assert_eq!(extract_token(&setup).unwrap(), "jwt");
    }

    #[test]
    fn missing_token_is_reported() {
        // Arrange
        let setup = client_setup(vec![]);

        // Act / Assert
        assert_eq!(extract_token(&setup), Err(SetupTokenError::Missing));
    }

    #[test]
    fn register_alias_type_is_rejected() {
        // Arrange
        let setup = client_setup(vec![AuthorizationToken::Register {
            token_alias: 1,
            token_type: 0,
            token_value: Bytes::from_static(b"jwt"),
        }]);

        // Act / Assert
        assert_eq!(
            extract_token(&setup),
            Err(SetupTokenError::UnsupportedAliasType)
        );
    }

    #[test]
    fn non_zero_token_type_is_rejected() {
        // Arrange
        let setup = client_setup(vec![AuthorizationToken::UseValue {
            token_type: 1,
            token_value: Bytes::from_static(b"jwt"),
        }]);

        // Act / Assert
        assert_eq!(
            extract_token(&setup),
            Err(SetupTokenError::UnsupportedTokenType(1))
        );
    }

    #[test]
    fn non_utf8_value_is_rejected() {
        // Arrange
        let setup = client_setup(vec![AuthorizationToken::UseValue {
            token_type: 0,
            token_value: Bytes::from_static(&[0xff, 0xfe]),
        }]);

        // Act / Assert
        assert_eq!(extract_token(&setup), Err(SetupTokenError::NotUtf8));
    }
}
