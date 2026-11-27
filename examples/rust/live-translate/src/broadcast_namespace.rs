pub(crate) const TRANSLATED_SUFFIX: &str = "translated";

/// Where the broadcasts live: `{root}/{id}` is a broadcast and
/// `{root}/{id}/translated` its translation. The root is the public
/// `anon/live-translate` or a temporary app's `{appId}/live-translate`.
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
}
