use std::str::FromStr;

use anyhow::{Error, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullTrackName {
    pub namespace: String,
    pub name: String,
}

impl FromStr for FullTrackName {
    type Err = Error;

    fn from_str(full: &str) -> Result<Self> {
        match full.rsplit_once('/') {
            Some((namespace, name)) if !namespace.is_empty() && !name.is_empty() => {
                Ok(FullTrackName {
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                })
            }
            _ => bail!("full track name must be <namespace>/<track>, got: {full}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_namespace_and_track() {
        let parsed: FullTrackName = "tokyo/cam01/video".parse().unwrap();

        assert_eq!(parsed.namespace, "tokyo/cam01");
        assert_eq!(parsed.name, "video");
    }

    #[test]
    fn rejects_name_without_namespace() {
        assert!("video".parse::<FullTrackName>().is_err());
    }
}
