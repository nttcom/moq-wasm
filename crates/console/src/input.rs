use serde::Deserialize;

use crate::apps::{MAX_TOKEN_TTL_SECONDS, TokenScope};

const MAX_NAME_CHARS: usize = 64;
const MAX_PATH_CHARS: usize = 255;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InvalidInput(pub(crate) &'static str);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateAppRequest {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct IssueTokenRequest {
    publish: Option<String>,
    subscribe: Option<String>,
    ttl_seconds: u64,
}

impl CreateAppRequest {
    pub(crate) fn name(&self) -> Result<String, InvalidInput> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS || has_control(name) {
            return Err(InvalidInput("invalid_name"));
        }
        Ok(name.to_owned())
    }
}

impl IssueTokenRequest {
    pub(crate) fn scope(&self) -> Result<TokenScope, InvalidInput> {
        if self.publish.is_none() && self.subscribe.is_none() {
            return Err(InvalidInput("no_scope"));
        }
        if self.ttl_seconds == 0 || self.ttl_seconds > MAX_TOKEN_TTL_SECONDS {
            return Err(InvalidInput("invalid_ttl"));
        }
        Ok(TokenScope {
            publish: namespace_path(self.publish.as_deref())?,
            subscribe: namespace_path(self.subscribe.as_deref())?,
            ttl_seconds: self.ttl_seconds,
        })
    }
}

fn namespace_path(path: Option<&str>) -> Result<Option<String>, InvalidInput> {
    let Some(path) = path else {
        return Ok(None);
    };
    let well_formed = path.is_empty() || path.split('/').all(|element| !element.is_empty());
    if !well_formed || path.chars().count() > MAX_PATH_CHARS || has_control(path) {
        return Err(InvalidInput("invalid_path"));
    }
    Ok(Some(path.to_owned()))
}

fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_request(
        publish: Option<&str>,
        subscribe: Option<&str>,
        ttl: u64,
    ) -> IssueTokenRequest {
        IssueTokenRequest {
            publish: publish.map(Into::into),
            subscribe: subscribe.map(Into::into),
            ttl_seconds: ttl,
        }
    }

    fn app_request(name: &str) -> CreateAppRequest {
        CreateAppRequest { name: name.into() }
    }

    #[test]
    fn a_name_is_trimmed() {
        // Act / Assert
        assert_eq!(app_request("  camera  ").name(), Ok("camera".to_owned()));
    }

    #[test]
    fn a_name_must_be_one_to_64_characters_without_control_characters() {
        // Act
        let blank = app_request("   ").name();
        let too_long = app_request(&"あ".repeat(65)).name();
        let with_newline = app_request("line\nbreak").name();
        let longest = app_request(&"あ".repeat(64)).name();

        // Assert
        assert_eq!(blank, Err(InvalidInput("invalid_name")));
        assert_eq!(too_long, Err(InvalidInput("invalid_name")));
        assert_eq!(with_newline, Err(InvalidInput("invalid_name")));
        assert!(longest.is_ok());
    }

    #[test]
    fn an_empty_path_grants_the_whole_app() {
        // Act
        let scope = token_request(Some(""), None, 3600).scope().unwrap();

        // Assert
        assert_eq!(scope.publish.as_deref(), Some(""));
        assert_eq!(scope.subscribe, None);
    }

    #[test]
    fn a_path_with_an_empty_element_is_rejected() {
        // Act / Assert
        for path in ["/site1", "site1/", "site1//cam1"] {
            assert_eq!(
                token_request(Some(path), None, 3600).scope().err(),
                Some(InvalidInput("invalid_path")),
                "{path}"
            );
        }
    }

    #[test]
    fn a_token_needs_publish_or_subscribe() {
        // Act / Assert
        assert_eq!(
            token_request(None, None, 3600).scope().err(),
            Some(InvalidInput("no_scope"))
        );
    }

    #[test]
    fn the_lifetime_is_one_second_to_24_hours() {
        // Act
        let zero = token_request(Some(""), None, 0).scope().err();
        let too_long = token_request(Some(""), None, MAX_TOKEN_TTL_SECONDS + 1)
            .scope()
            .err();
        let longest = token_request(Some(""), None, MAX_TOKEN_TTL_SECONDS).scope();

        // Assert
        assert_eq!(zero, Some(InvalidInput("invalid_ttl")));
        assert_eq!(too_long, Some(InvalidInput("invalid_ttl")));
        assert!(longest.is_ok());
    }
}
