use std::time::SystemTime;

pub(crate) type NamespacePath = Vec<String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedToken {
    pub(crate) app_id: String,
    pub(crate) publish: Option<NamespacePath>,
    pub(crate) subscribe: Option<NamespacePath>,
    pub(crate) is_relay: bool,
    pub(crate) expires_at: Option<SystemTime>,
}

impl VerifiedToken {
    pub(crate) fn full_access() -> Self {
        Self {
            app_id: String::new(),
            publish: Some(vec![]),
            subscribe: Some(vec![]),
            is_relay: true,
            expires_at: None,
        }
    }
}

pub(crate) fn parse_namespace_path(claim: &str) -> NamespacePath {
    if claim.is_empty() {
        return vec![];
    }
    claim.split('/').map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::parse_namespace_path;

    #[test]
    fn empty_claim_is_the_app_root() {
        // Act / Assert
        assert!(parse_namespace_path("").is_empty());
    }

    #[test]
    fn claim_is_split_on_slash() {
        // Act / Assert
        assert_eq!(parse_namespace_path("site1/cam1"), vec!["site1", "cam1"]);
    }
}
