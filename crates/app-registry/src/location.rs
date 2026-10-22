use std::path::PathBuf;

use anyhow::bail;

use crate::cloud_storage::Endpoint;

const DEFAULT_OBJECT: &str = "registry.json";

pub enum Location {
    CloudStorage {
        endpoint: Endpoint,
        bucket: String,
        object: String,
    },
    File(PathBuf),
}

pub struct LocationVariables<'a> {
    pub file: &'a str,
    pub bucket: &'a str,
    pub object: &'a str,
}

impl Location {
    /// Returns `None` when the bucket and file variables are both unset or empty. Fails when both
    /// are set, or when `STORAGE_EMULATOR_HOST` is set but is not a URL.
    pub fn from_env(variables: &LocationVariables) -> anyhow::Result<Option<Self>> {
        match (
            non_empty_env(variables.bucket),
            non_empty_env(variables.file),
        ) {
            (Some(_), Some(_)) => bail!("set {} or {}, not both", variables.bucket, variables.file),
            (Some(bucket), None) => Ok(Some(Self::CloudStorage {
                endpoint: Endpoint::from_env()?,
                bucket,
                object: non_empty_env(variables.object)
                    .unwrap_or_else(|| DEFAULT_OBJECT.to_owned()),
            })),
            (None, Some(file)) => Ok(Some(Self::File(PathBuf::from(file)))),
            (None, None) => Ok(None),
        }
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
