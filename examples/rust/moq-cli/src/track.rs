use std::str::FromStr;

use anyhow::{Error, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullTrackName {
    pub namespace: Namespace,
    pub name: String,
}

/// A track namespace written with `/` between its elements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace(pub Vec<String>);

impl FromStr for Namespace {
    type Err = Error;

    fn from_str(namespace: &str) -> Result<Self> {
        let elements: Vec<String> = namespace.split('/').map(str::to_string).collect();
        if elements.iter().any(String::is_empty) {
            bail!("namespace must be non-empty elements joined by '/', got: {namespace}");
        }
        Ok(Namespace(elements))
    }
}

impl FromStr for FullTrackName {
    type Err = Error;

    fn from_str(full: &str) -> Result<Self> {
        match full.rsplit_once('/') {
            Some((namespace, name)) if !namespace.is_empty() && !name.is_empty() => {
                Ok(FullTrackName {
                    namespace: namespace.parse()?,
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

        assert_eq!(parsed.namespace.0, ["tokyo", "cam01"]);
        assert_eq!(parsed.name, "video");
    }

    #[test]
    fn splits_a_namespace_into_its_elements() {
        // Arrange
        let namespace = "APP/tokyo/cam01";

        // Act
        let parsed: Namespace = namespace.parse().unwrap();

        // Assert
        assert_eq!(parsed.0, ["APP", "tokyo", "cam01"]);
    }

    #[test]
    fn rejects_a_namespace_with_an_empty_element() {
        // Act / Assert
        assert!("APP//cam01".parse::<Namespace>().is_err());
    }

    #[test]
    fn rejects_name_without_namespace() {
        assert!("video".parse::<FullTrackName>().is_err());
    }
}
