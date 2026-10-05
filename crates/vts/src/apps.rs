use std::{collections::HashMap, path::Path};

use anyhow::{Context, bail};
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct App {
    pub secret: String,
    pub is_relay: bool,
}

pub type Apps = HashMap<String, App>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    app_id: String,
    #[serde(flatten)]
    app: App,
}

pub fn parse_apps(text: &str) -> anyhow::Result<Apps> {
    let entries: Vec<Entry> =
        serde_json::from_str(text).context("apps file must be a JSON array of apps")?;
    let mut apps = Apps::new();
    for (index, entry) in entries.into_iter().enumerate() {
        if entry.app_id.is_empty() {
            bail!("apps[{index}].appId must be a non-empty string");
        }
        if entry.app.secret.is_empty() {
            bail!("apps[{index}].secret must be a non-empty string");
        }
        if apps.contains_key(&entry.app_id) {
            bail!("apps[{index}].appId {:?} is duplicated", entry.app_id);
        }
        apps.insert(entry.app_id, entry.app);
    }
    Ok(apps)
}

pub async fn load_apps(path: &Path) -> anyhow::Result<Apps> {
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read apps file {}", path.display()))?;
    parse_apps(&text)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_entries_into_a_map_keyed_by_app_id() {
        // Arrange
        let text = json!([
            { "appId": "a", "secret": "s1", "isRelay": false },
            { "appId": "r", "secret": "s2", "isRelay": true },
        ])
        .to_string();

        // Act
        let apps = parse_apps(&text).unwrap();

        // Assert
        assert_eq!(
            apps["a"],
            App {
                secret: "s1".into(),
                is_relay: false
            }
        );
        assert_eq!(
            apps["r"],
            App {
                secret: "s2".into(),
                is_relay: true
            }
        );
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
        let error = parse_apps(&text).unwrap_err();

        // Assert
        assert!(error.to_string().contains("duplicated"), "{error}");
    }

    #[test]
    fn rejects_a_missing_secret() {
        // Arrange
        let text = json!([{ "appId": "a", "isRelay": false }]).to_string();

        // Act
        let error = parse_apps(&text).unwrap_err();

        // Assert
        assert!(format!("{error:#}").contains("secret"), "{error:#}");
    }

    #[test]
    fn rejects_an_empty_secret() {
        // Arrange
        let text = json!([{ "appId": "a", "secret": "", "isRelay": false }]).to_string();

        // Act
        let error = parse_apps(&text).unwrap_err();

        // Assert
        assert!(error.to_string().contains("secret"), "{error}");
    }

    #[test]
    fn rejects_a_non_boolean_is_relay() {
        // Arrange
        let text = json!([{ "appId": "a", "secret": "s", "isRelay": "yes" }]).to_string();

        // Act / Assert
        assert!(parse_apps(&text).is_err());
    }

    #[test]
    fn rejects_a_document_that_is_not_an_array() {
        // Act
        let error = parse_apps("{}").unwrap_err();

        // Assert
        assert!(error.to_string().contains("array"), "{error}");
    }
}
