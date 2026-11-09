const TRANSLATED_SUFFIX: &str = "translated";

/// Where the broadcasts live: `{root}/{id}` is a broadcast and
/// `{root}/{id}/translated` its translation. The root is the served app's id,
/// whose token the relay checks, or the public `anon/live-translate`.
#[derive(Clone)]
pub(crate) struct BroadcastNamespaces {
    root: String,
}

impl BroadcastNamespaces {
    pub(crate) const PUBLIC_ROOT: &'static str = "anon/live-translate";

    pub(crate) fn new(root: String) -> Self {
        Self { root }
    }

    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    pub(crate) fn is_broadcast(&self, namespace: &str) -> bool {
        namespace
            .strip_prefix(self.root.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
    }

    pub(crate) fn broadcast_of_translated<'a>(&self, namespace: &'a str) -> Option<&'a str> {
        namespace
            .strip_suffix(TRANSLATED_SUFFIX)
            .and_then(|rest| rest.strip_suffix('/'))
            .filter(|broadcast| self.is_broadcast(broadcast))
    }
}

pub(crate) fn translated_namespace(broadcast_namespace: &str) -> String {
    format!("{broadcast_namespace}/{TRANSLATED_SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public() -> BroadcastNamespaces {
        BroadcastNamespaces::new(BroadcastNamespaces::PUBLIC_ROOT.to_string())
    }

    #[test]
    fn direct_child_of_the_root_is_a_broadcast() {
        // Act / Assert
        assert!(public().is_broadcast("anon/live-translate/brave-otter"));
        assert!(BroadcastNamespaces::new("app-1".to_string()).is_broadcast("app-1/brave-otter"));
    }

    #[test]
    fn own_translated_namespace_is_not_a_broadcast() {
        // Act / Assert
        assert!(!public().is_broadcast("anon/live-translate/brave-otter/translated"));
    }

    #[test]
    fn root_itself_and_lookalike_roots_are_not_broadcasts() {
        // Act / Assert
        assert!(!public().is_broadcast("anon/live-translate"));
        assert!(!public().is_broadcast("anon/live-translate/"));
        assert!(!public().is_broadcast("anon/live-translate-old/brave-otter"));
    }

    #[test]
    fn translated_namespace_leads_back_to_its_broadcast() {
        // Act / Assert
        assert_eq!(
            public().broadcast_of_translated("anon/live-translate/brave-otter/translated"),
            Some("anon/live-translate/brave-otter")
        );
        assert_eq!(
            public().broadcast_of_translated("anon/live-translate/brave-otter"),
            None
        );
        assert_eq!(
            public().broadcast_of_translated("anon/other/brave-otter/translated"),
            None
        );
    }

    #[test]
    fn translated_namespace_is_nested_under_the_broadcast() {
        // Act / Assert
        assert_eq!(
            translated_namespace("anon/live-translate/brave-otter"),
            "anon/live-translate/brave-otter/translated"
        );
    }
}
