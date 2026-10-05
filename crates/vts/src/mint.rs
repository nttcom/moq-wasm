use std::time::Duration;

use anyhow::bail;
use serde::Serialize;

use crate::{apps::Apps, jwt::sign_token};

#[derive(clap::Args)]
pub struct MintRequest {
    /// appId of the row whose secret signs the token
    #[arg(long)]
    pub app_id: String,

    /// Relative namespace path the token may publish under; "" grants everything under the appId
    #[arg(long)]
    pub publish: Option<String>,

    /// Relative namespace path the token may subscribe under; "" grants everything under the appId
    #[arg(long)]
    pub subscribe: Option<String>,

    /// Token lifetime, e.g. 30m, 12h, 365d
    #[arg(long, default_value = "12h", value_parser = parse_ttl)]
    pub ttl: Duration,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Claims<'a> {
    app_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    publish: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subscribe: Option<&'a str>,
    iat: u64,
    exp: u64,
}

pub fn mint_token(apps: &Apps, request: &MintRequest, now: u64) -> anyhow::Result<String> {
    let Some(app) = apps.get(&request.app_id) else {
        bail!("unknown appId {:?}", request.app_id);
    };
    let claims = Claims {
        app_id: &request.app_id,
        publish: request.publish.as_deref(),
        subscribe: request.subscribe.as_deref(),
        iat: now,
        exp: now + request.ttl.as_secs(),
    };
    sign_token(&claims, &app.secret)
}

fn parse_ttl(text: &str) -> Result<Duration, String> {
    let invalid = || format!("invalid ttl {text:?}, expected e.g. 30m, 12h, 365d");
    let unit_seconds = match text.as_bytes().last() {
        Some(b's') => 1,
        Some(b'm') => 60,
        Some(b'h') => 3600,
        Some(b'd') => 86400,
        _ => return Err(invalid()),
    };
    let count = text[..text.len() - 1]
        .parse::<u64>()
        .map_err(|_| invalid())?;
    count
        .checked_mul(unit_seconds)
        .map(Duration::from_secs)
        .ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        test_support::{APP_ID, NOW, test_apps},
        verify::verify_token,
    };

    fn request(app_id: &str) -> MintRequest {
        MintRequest {
            app_id: app_id.to_owned(),
            publish: Some("site1/cam1".to_owned()),
            subscribe: Some(String::new()),
            ttl: Duration::from_secs(30 * 60),
        }
    }

    #[test]
    fn minted_token_verifies_with_the_requested_claims() {
        // Arrange
        let apps = test_apps();

        // Act
        let token = mint_token(&apps, &request(APP_ID), NOW).unwrap();
        let verified = verify_token(&token, &apps).unwrap();

        // Assert
        assert_eq!(
            serde_json::Value::Object(verified.claims),
            json!({
                "appId": APP_ID,
                "publish": "site1/cam1",
                "subscribe": "",
                "iat": NOW,
                "exp": NOW + 30 * 60,
            })
        );
    }

    #[test]
    fn omitted_publish_and_subscribe_are_left_out_of_the_claims() {
        // Arrange
        let apps = test_apps();
        let request = MintRequest {
            publish: None,
            subscribe: None,
            ..request(APP_ID)
        };

        // Act
        let token = mint_token(&apps, &request, NOW).unwrap();
        let verified = verify_token(&token, &apps).unwrap();

        // Assert
        assert!(!verified.claims.contains_key("publish"));
        assert!(!verified.claims.contains_key("subscribe"));
    }

    #[test]
    fn unknown_app_id_is_an_error() {
        // Act
        let error = mint_token(&test_apps(), &request("NOBODY"), NOW).unwrap_err();

        // Assert
        assert!(error.to_string().contains("NOBODY"), "{error}");
    }

    #[test]
    fn parses_ttl_units() {
        // Act / Assert
        assert_eq!(parse_ttl("15s"), Ok(Duration::from_secs(15)));
        assert_eq!(parse_ttl("30m"), Ok(Duration::from_secs(30 * 60)));
        assert_eq!(parse_ttl("12h"), Ok(Duration::from_secs(12 * 3600)));
        assert_eq!(parse_ttl("365d"), Ok(Duration::from_secs(365 * 86400)));
    }

    #[test]
    fn rejects_ttl_without_a_unit_or_number() {
        // Act / Assert
        assert!(parse_ttl("12").is_err());
        assert!(parse_ttl("h").is_err());
        assert!(parse_ttl("1.5h").is_err());
        assert!(parse_ttl("").is_err());
    }
}
