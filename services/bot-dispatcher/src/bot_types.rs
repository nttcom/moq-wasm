use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, ensure};
use google_cloud::MetadataToken;
use job_launcher::JobLauncher;
use serde::Deserialize;
use serde_json::Value;

/// What one kind of bot is and how it runs, read from the types file.
pub(crate) struct BotType {
    pub(crate) launcher: JobLauncher,
    /// The job's arguments before the ones `bot-dispatcher` adds.
    pub(crate) args: Vec<String>,
    /// How long the room may have no human member before the bot ends.
    pub(crate) grace: Duration,
    pub(crate) max_run: Duration,
    /// Bots of this type running at once, over every app.
    pub(crate) max_running: usize,
    /// The values each option may take; `None` forwards any options.
    options: Option<BTreeMap<String, Vec<Value>>>,
}

impl BotType {
    pub(crate) fn is_local(&self) -> bool {
        matches!(self.launcher, JobLauncher::Local { .. })
    }

    /// Whether a launch may pass `options` to the bot: absent options, or an
    /// object whose every key the type's table names with one of its values.
    pub(crate) fn accepts(&self, options: Option<&Value>) -> bool {
        let (Some(table), Some(options)) = (&self.options, options) else {
            return true;
        };
        let Value::Object(options) = options else {
            return false;
        };
        options.iter().all(|(name, value)| {
            table
                .get(name)
                .is_some_and(|allowed| allowed.contains(value))
        })
    }
}

pub(crate) struct BotTypes(BTreeMap<String, BotType>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BotTypeEntry {
    launcher: LauncherEntry,
    #[serde(default)]
    args: Vec<String>,
    grace_seconds: u64,
    max_seconds: u64,
    max_running: usize,
    #[serde(default)]
    options: Option<BTreeMap<String, Vec<Value>>>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum LauncherEntry {
    Local { program: PathBuf },
    CloudRun { job: String },
}

impl BotTypes {
    /// A JSON object from type name to its entry. A type name is one
    /// namespace element of `[a-z0-9-]`, since a bot publishes under
    /// `<room>/bots/<type>`.
    pub(crate) fn parse(text: &str, metadata_token: &Arc<MetadataToken>) -> Result<Self> {
        let entries: BTreeMap<String, BotTypeEntry> =
            serde_json::from_str(text).context("invalid bot types")?;
        let client = reqwest::Client::new();
        let mut types = BTreeMap::new();
        for (name, entry) in entries {
            ensure!(
                !name.is_empty()
                    && name.bytes().all(|byte| byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'-'),
                "bot type {name:?} is not one element of [a-z0-9-]"
            );
            for (option, values) in entry.options.iter().flatten() {
                ensure!(
                    !values.is_empty()
                        && values.iter().all(|value| matches!(
                            value,
                            Value::String(_) | Value::Bool(_) | Value::Number(_)
                        )),
                    "option {option:?} of bot type {name:?} is not a non-empty array of strings, booleans or numbers"
                );
            }
            let launcher = match entry.launcher {
                LauncherEntry::Local { program } => JobLauncher::Local { program },
                LauncherEntry::CloudRun { job } => JobLauncher::CloudRun {
                    job,
                    client: client.clone(),
                    metadata_token: Arc::clone(metadata_token),
                },
            };
            types.insert(
                name,
                BotType {
                    launcher,
                    args: entry.args,
                    grace: Duration::from_secs(entry.grace_seconds),
                    max_run: Duration::from_secs(entry.max_seconds),
                    max_running: entry.max_running,
                    options: entry.options,
                },
            );
        }
        Ok(Self(types))
    }

    pub(crate) fn get(&self, name: &str) -> Option<&BotType> {
        self.0.get(name)
    }

    /// The type names, in order.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<BotTypes> {
        BotTypes::parse(text, &Arc::new(MetadataToken::new(reqwest::Client::new())))
    }

    #[test]
    fn reads_each_type_with_its_launcher() {
        // Arrange
        let text = r#"{
            "recorder": {"launcher": {"kind": "cloud-run", "job": "projects/p/locations/r/jobs/rec"},
                         "args": ["record"], "grace_seconds": 300, "max_seconds": 7200, "max_running": 5},
            "chat": {"launcher": {"kind": "local", "program": "/bin/chat"},
                     "grace_seconds": 30, "max_seconds": 3600, "max_running": 1}
        }"#;

        // Act
        let types = parse(text).unwrap();

        // Assert
        let recorder = types.get("recorder").unwrap();
        assert!(!recorder.is_local());
        assert_eq!(recorder.args, ["record"]);
        assert_eq!(recorder.grace, Duration::from_secs(300));
        assert!(types.get("chat").unwrap().is_local());
    }

    #[test]
    fn a_type_name_must_be_one_namespace_element() {
        // Arrange
        let text = r#"{"a/b": {"launcher": {"kind": "local", "program": "/bin/x"},
                       "grace_seconds": 1, "max_seconds": 1, "max_running": 1}}"#;

        // Act / Assert
        assert!(parse(text).is_err());
    }

    fn with_options(options: &str) -> Result<BotTypes> {
        parse(&format!(
            r#"{{"recorder": {{"launcher": {{"kind": "local", "program": "/bin/x"}},
                 "grace_seconds": 1, "max_seconds": 1, "max_running": 1, "options": {options}}}}}"#
        ))
    }

    #[test]
    fn an_options_table_maps_each_option_to_scalar_values() {
        // Act / Assert
        assert!(
            with_options(r#"{"layout": ["combined"], "composite": [true, false], "fps": [30]}"#)
                .is_ok()
        );
        for invalid in [
            r#"[]"#,
            r#"{"layout": "combined"}"#,
            r#"{"layout": []}"#,
            r#"{"layout": [null]}"#,
            r#"{"layout": [["combined"]]}"#,
            r#"{"layout": [{"name": "combined"}]}"#,
        ] {
            assert!(with_options(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn a_type_with_a_table_accepts_only_the_options_it_names_with_their_values() {
        // Arrange
        let types =
            with_options(r#"{"layout": ["combined", "per-track"], "composite": [true, false]}"#)
                .unwrap();
        let recorder = types.get("recorder").unwrap();

        // Act / Assert
        assert!(recorder.accepts(None));
        assert!(recorder.accepts(Some(&serde_json::json!({}))));
        assert!(recorder.accepts(Some(&serde_json::json!({ "layout": "per-track" }))));
        assert!(recorder.accepts(Some(
            &serde_json::json!({ "layout": "combined", "composite": true })
        )));
        assert!(!recorder.accepts(Some(&serde_json::json!(["layout"]))));
        assert!(!recorder.accepts(Some(&serde_json::json!({ "format": "mp4" }))));
        assert!(!recorder.accepts(Some(&serde_json::json!({ "layout": "grid" }))));
        assert!(!recorder.accepts(Some(&serde_json::json!({ "composite": "true" }))));
    }

    #[test]
    fn a_type_without_a_table_accepts_any_options() {
        // Arrange
        let types = parse(
            r#"{"chat": {"launcher": {"kind": "local", "program": "/bin/chat"},
                         "grace_seconds": 30, "max_seconds": 3600, "max_running": 1}}"#,
        )
        .unwrap();

        // Act / Assert
        assert!(
            types
                .get("chat")
                .unwrap()
                .accepts(Some(&serde_json::json!("anything")))
        );
    }
}
