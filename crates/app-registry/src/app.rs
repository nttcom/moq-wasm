use std::collections::HashSet;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct App {
    pub app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub secret: String,
    pub is_relay: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotated_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<Phone>,
}

/// The app's phone settings on the SIP gateway
/// (docs/architecture/sip-gateway/architecture.md). Every number and
/// destination prefix is `+` and 1 to 15 digits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Phone {
    /// Numbers whose inbound calls reach this app; one app per number.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub numbers: Vec<String>,
    /// URL of the app server the gateway asks where to bridge an inbound
    /// call; required when `numbers` is not empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_webhook: Option<String>,
    /// Prefixes of the numbers the app may call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_destinations: Vec<String>,
}

impl Phone {
    /// Whether `number` is E.164 and starts with one of `allowed_destinations`.
    pub fn allows_destination(&self, number: &str) -> bool {
        is_e164(number)
            && self
                .allowed_destinations
                .iter()
                .any(|prefix| number.starts_with(prefix.as_str()))
    }

    fn validate(&self, index: usize) -> anyhow::Result<()> {
        for number in self.numbers.iter().chain(&self.allowed_destinations) {
            if !is_e164(number) {
                bail!("apps[{index}].phone: {number:?} is not + and 1 to 15 digits");
            }
        }
        if !self.numbers.is_empty() && self.call_webhook.as_deref().is_none_or(str::is_empty) {
            bail!("apps[{index}].phone.callWebhook is required with numbers");
        }
        Ok(())
    }
}

/// Whether `number` is `+` and 1 to 15 digits (ITU-T E.164).
pub fn is_e164(number: &str) -> bool {
    number.strip_prefix('+').is_some_and(|digits| {
        (1..=15).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Apps(Vec<App>);

impl Apps {
    pub fn new(apps: Vec<App>) -> anyhow::Result<Self> {
        let mut app_ids = HashSet::new();
        let mut numbers = HashSet::new();
        for (index, app) in apps.iter().enumerate() {
            if app.app_id.is_empty() {
                bail!("apps[{index}].appId must be a non-empty string");
            }
            if app.secret.is_empty() {
                bail!("apps[{index}].secret must be a non-empty string");
            }
            if !app_ids.insert(app.app_id.as_str()) {
                bail!("apps[{index}].appId {:?} is duplicated", app.app_id);
            }
            if let Some(phone) = &app.phone {
                phone.validate(index)?;
                for number in &phone.numbers {
                    if !numbers.insert(number.as_str()) {
                        bail!("apps[{index}].phone number {number} belongs to another app");
                    }
                }
            }
        }
        Ok(Self(apps))
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let apps =
            serde_json::from_str(text).context("the registry must be a JSON array of apps")?;
        Self::new(apps)
    }

    pub fn to_json(&self) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(&self.0)?)
    }

    pub fn get(&self, app_id: &str) -> Option<&App> {
        self.0.iter().find(|app| app.app_id == app_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &App> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn into_vec(self) -> Vec<App> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_rows_with_only_the_original_three_fields() {
        // Arrange
        let text = json!([
            { "appId": "a", "secret": "s1", "isRelay": false },
            { "appId": "r", "secret": "s2", "isRelay": true },
        ])
        .to_string();

        // Act
        let apps = Apps::parse(&text).unwrap();

        // Assert
        assert_eq!(
            apps.get("r"),
            Some(&App {
                app_id: "r".into(),
                name: None,
                secret: "s2".into(),
                is_relay: true,
                sub: None,
                created_at: None,
                rotated_at: None,
                phone: None,
            })
        );
    }

    #[test]
    fn serialisation_round_trips_every_field_and_omits_absent_ones() {
        // Arrange
        let apps = Apps::new(vec![
            App {
                app_id: "a".into(),
                name: Some("camera".into()),
                secret: "s1".into(),
                is_relay: false,
                sub: Some("accounts.google.com:1".into()),
                created_at: Some(1),
                rotated_at: Some(2),
                phone: Some(Phone {
                    numbers: vec!["+815012345678".into()],
                    call_webhook: Some("https://app.example/calls".into()),
                    allowed_destinations: vec!["+81".into()],
                }),
            },
            App {
                app_id: "r".into(),
                name: None,
                secret: "s2".into(),
                is_relay: true,
                sub: None,
                created_at: None,
                rotated_at: None,
                phone: None,
            },
        ])
        .unwrap();

        // Act
        let json: serde_json::Value = serde_json::from_slice(&apps.to_json().unwrap()).unwrap();

        // Assert
        assert_eq!(
            json,
            json!([
                { "appId": "a", "name": "camera", "secret": "s1", "isRelay": false,
                  "sub": "accounts.google.com:1", "createdAt": 1, "rotatedAt": 2,
                  "phone": { "numbers": ["+815012345678"],
                             "callWebhook": "https://app.example/calls",
                             "allowedDestinations": ["+81"] } },
                { "appId": "r", "secret": "s2", "isRelay": true },
            ])
        );
        assert_eq!(Apps::parse(&json.to_string()).unwrap(), apps);
    }

    #[test]
    fn rejects_a_duplicated_app_id() {
        // Arrange
        let text = json!([
            { "appId": "a", "secret": "s1", "isRelay": false },
            { "appId": "a", "secret": "s2", "isRelay": false },
        ])
        .to_string();

        // Act
        let error = Apps::parse(&text).unwrap_err();

        // Assert
        assert!(error.to_string().contains("duplicated"), "{error}");
    }

    #[test]
    fn one_sub_may_own_several_apps() {
        // Arrange
        let text = json!([
            { "appId": "a", "secret": "s1", "isRelay": false, "sub": "accounts.google.com:1" },
            { "appId": "b", "secret": "s2", "isRelay": false, "sub": "accounts.google.com:1" },
        ])
        .to_string();

        // Act / Assert
        assert_eq!(Apps::parse(&text).unwrap().len(), 2);
    }

    #[test]
    fn rejects_a_missing_secret() {
        // Arrange
        let text = json!([{ "appId": "a", "isRelay": false }]).to_string();

        // Act
        let error = Apps::parse(&text).unwrap_err();

        // Assert
        assert!(format!("{error:#}").contains("secret"), "{error:#}");
    }

    #[test]
    fn rejects_an_empty_secret() {
        // Arrange
        let text = json!([{ "appId": "a", "secret": "", "isRelay": false }]).to_string();

        // Act
        let error = Apps::parse(&text).unwrap_err();

        // Assert
        assert!(error.to_string().contains("secret"), "{error}");
    }

    #[test]
    fn rejects_a_non_boolean_is_relay() {
        // Arrange
        let text = json!([{ "appId": "a", "secret": "s", "isRelay": "yes" }]).to_string();

        // Act / Assert
        assert!(Apps::parse(&text).is_err());
    }

    #[test]
    fn rejects_a_phone_number_that_is_not_e164() {
        // Arrange
        let with_number = |number: &str| {
            json!([{ "appId": "a", "secret": "s", "isRelay": false,
                     "phone": { "allowedDestinations": [number] } }])
            .to_string()
        };

        // Act / Assert: `+` alone would allow every destination
        assert!(Apps::parse(&with_number("+81")).is_ok());
        for number in ["+", "81", "+81-3", "+1234567890123456"] {
            assert!(Apps::parse(&with_number(number)).is_err(), "{number}");
        }
    }

    #[test]
    fn rejects_numbers_without_a_webhook() {
        // Arrange
        let text = json!([{ "appId": "a", "secret": "s", "isRelay": false,
                            "phone": { "numbers": ["+815012345678"] } }])
        .to_string();

        // Act
        let error = Apps::parse(&text).unwrap_err();

        // Assert
        assert!(error.to_string().contains("callWebhook"), "{error}");
    }

    #[test]
    fn rejects_a_number_two_apps_claim() {
        // Arrange
        let phone =
            json!({ "numbers": ["+815012345678"], "callWebhook": "https://app.example/calls" });
        let text = json!([
            { "appId": "a", "secret": "s1", "isRelay": false, "phone": phone },
            { "appId": "b", "secret": "s2", "isRelay": false, "phone": phone },
        ])
        .to_string();

        // Act
        let error = Apps::parse(&text).unwrap_err();

        // Assert
        assert!(error.to_string().contains("another app"), "{error}");
    }

    #[test]
    fn allows_only_destinations_under_an_allowed_prefix() {
        // Arrange
        let phone = Phone {
            numbers: Vec::new(),
            call_webhook: None,
            allowed_destinations: vec!["+8150".into(), "+1".into()],
        };

        // Act / Assert
        assert!(phone.allows_destination("+815012345678"));
        assert!(phone.allows_destination("+12025550123"));
        assert!(!phone.allows_destination("+819012345678"));
        assert!(!phone.allows_destination("+8150abc"));
    }

    #[test]
    fn rejects_a_document_that_is_not_an_array() {
        // Act
        let error = Apps::parse("{}").unwrap_err();

        // Assert
        assert!(error.to_string().contains("array"), "{error}");
    }
}
