use moqt::wire::AuthorizationToken;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TokenParameterError {
    #[error("no AUTHORIZATION TOKEN parameter")]
    Missing,
    #[error("AUTHORIZATION TOKEN alias type is not USE_VALUE")]
    UnsupportedAliasType,
    #[error("AUTHORIZATION TOKEN type {0} is not supported (expected 0)")]
    UnsupportedTokenType(u64),
    #[error("AUTHORIZATION TOKEN value is not UTF-8")]
    NotUtf8,
}

pub(crate) fn extract_token(
    authorization_tokens: &[AuthorizationToken],
) -> Result<String, TokenParameterError> {
    let token = authorization_tokens
        .first()
        .ok_or(TokenParameterError::Missing)?;
    match token {
        AuthorizationToken::UseValue {
            token_type: 0,
            token_value,
        } => String::from_utf8(token_value.to_vec()).map_err(|_| TokenParameterError::NotUtf8),
        AuthorizationToken::UseValue { token_type, .. } => {
            Err(TokenParameterError::UnsupportedTokenType(*token_type))
        }
        AuthorizationToken::Delete
        | AuthorizationToken::Register { .. }
        | AuthorizationToken::UseAlias { .. } => Err(TokenParameterError::UnsupportedAliasType),
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use moqt::wire::AuthorizationToken;

    use super::{TokenParameterError, extract_token};

    #[test]
    fn use_value_token_of_type_zero_is_returned_as_string() {
        // Arrange
        let tokens = [AuthorizationToken::use_value_utf8("jwt")];

        // Act / Assert
        assert_eq!(extract_token(&tokens).unwrap(), "jwt");
    }

    #[test]
    fn missing_token_is_reported() {
        // Arrange
        let tokens: [AuthorizationToken; 0] = [];

        // Act / Assert
        assert_eq!(extract_token(&tokens), Err(TokenParameterError::Missing));
    }

    #[test]
    fn register_alias_type_is_rejected() {
        // Arrange
        let tokens = [AuthorizationToken::Register {
            token_alias: 1,
            token_type: 0,
            token_value: Bytes::from_static(b"jwt"),
        }];

        // Act / Assert
        assert_eq!(
            extract_token(&tokens),
            Err(TokenParameterError::UnsupportedAliasType)
        );
    }

    #[test]
    fn non_zero_token_type_is_rejected() {
        // Arrange
        let tokens = [AuthorizationToken::UseValue {
            token_type: 1,
            token_value: Bytes::from_static(b"jwt"),
        }];

        // Act / Assert
        assert_eq!(
            extract_token(&tokens),
            Err(TokenParameterError::UnsupportedTokenType(1))
        );
    }

    #[test]
    fn non_utf8_value_is_rejected() {
        // Arrange
        let tokens = [AuthorizationToken::UseValue {
            token_type: 0,
            token_value: Bytes::from_static(&[0xff, 0xfe]),
        }];

        // Act / Assert
        assert_eq!(extract_token(&tokens), Err(TokenParameterError::NotUtf8));
    }
}
