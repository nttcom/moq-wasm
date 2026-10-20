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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Apps(Vec<App>);

impl Apps {
    pub fn new(apps: Vec<App>) -> anyhow::Result<Self> {
        let mut app_ids = HashSet::new();
        let mut subs = HashSet::new();
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
            if let Some(sub) = &app.sub
                && !subs.insert(sub.as_str())
            {
                bail!("apps[{index}].sub {sub:?} already owns another app");
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

    pub fn into_vec(self) -> Vec<App> {
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
            },
            App {
                app_id: "r".into(),
                name: None,
                secret: "s2".into(),
                is_relay: true,
                sub: None,
                created_at: None,
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
                  "sub": "accounts.google.com:1", "createdAt": 1 },
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
    fn rejects_a_second_app_for_the_same_sub() {
        // Arrange
        let text = json!([
            { "appId": "a", "secret": "s1", "isRelay": false, "sub": "accounts.google.com:1" },
            { "appId": "b", "secret": "s2", "isRelay": false, "sub": "accounts.google.com:1" },
        ])
        .to_string();

        // Act
        let error = Apps::parse(&text).unwrap_err();

        // Assert
        assert!(error.to_string().contains("sub"), "{error}");
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
    fn rejects_a_document_that_is_not_an_array() {
        // Act
        let error = Apps::parse("{}").unwrap_err();

        // Assert
        assert!(error.to_string().contains("array"), "{error}");
    }
}
