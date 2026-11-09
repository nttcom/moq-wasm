pub(crate) type TrackNamespace = String;
pub(crate) type TrackNamespacePrefix = String;

/// Whether `track_namespace_prefix` is the leading fields of `track_namespace`
/// (draft-14 §9.28), both `/`-joined: `a/b` matches `a/b` and `a/b/c`, not
/// `a/bc`, and the empty prefix, the empty tuple, leads every namespace. The
/// join keeps field boundaries because `authorize` rejects a field containing
/// `/`.
pub(crate) fn namespace_has_prefix(track_namespace: &str, track_namespace_prefix: &str) -> bool {
    track_namespace_prefix.is_empty()
        || track_namespace
            .strip_prefix(track_namespace_prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

pub(crate) fn namespace_prefixes_overlap(a: &str, b: &str) -> bool {
    namespace_has_prefix(a, b) || namespace_has_prefix(b, a)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TrackKey {
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
}

impl TrackKey {
    pub(crate) fn new(track_namespace: impl Into<String>, track_name: impl Into<String>) -> Self {
        Self {
            track_namespace: track_namespace.into(),
            track_name: track_name.into(),
        }
    }
}

impl std::fmt::Display for TrackKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.track_namespace, self.track_name)
    }
}

#[cfg(test)]
mod tests {
    use super::{namespace_has_prefix, namespace_prefixes_overlap};

    #[test]
    fn prefix_matches_the_namespace_itself_and_longer_namespaces() {
        // Act / Assert
        assert!(namespace_has_prefix("APP/site1", "APP/site1"));
        assert!(namespace_has_prefix("APP/site1/cam1", "APP/site1"));
    }

    #[test]
    fn prefix_does_not_match_a_field_it_only_starts() {
        // Act / Assert
        assert!(!namespace_has_prefix("APP/site10", "APP/site1"));
        assert!(!namespace_has_prefix("APP/site10/cam1", "APP/site1"));
        assert!(!namespace_has_prefix("APP", "APP/site1"));
        assert!(!namespace_has_prefix("APP//site1", "APP/site1"));
    }

    #[test]
    fn the_empty_prefix_leads_every_namespace() {
        // Act / Assert
        assert!(namespace_has_prefix("APP/site1", ""));
        assert!(namespace_has_prefix("", ""));
    }

    #[test]
    fn prefixes_overlap_when_either_leads_the_other() {
        // Act / Assert
        assert!(namespace_prefixes_overlap("APP/site1", "APP/site1/cam1"));
        assert!(namespace_prefixes_overlap("APP/site1/cam1", "APP/site1"));
        assert!(!namespace_prefixes_overlap("APP/site1", "APP/site10"));
    }
}
