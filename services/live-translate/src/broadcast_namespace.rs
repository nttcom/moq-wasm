pub(crate) const BROADCAST_PREFIX: &str = "anon/live-translate";
const TRANSLATED_SUFFIX: &str = "translated";

pub(crate) fn is_broadcast(namespace: &str) -> bool {
    namespace
        .strip_prefix(BROADCAST_PREFIX)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

pub(crate) fn translated_namespace(broadcast_namespace: &str) -> String {
    format!("{broadcast_namespace}/{TRANSLATED_SUFFIX}")
}

pub(crate) fn broadcast_of_translated(namespace: &str) -> Option<&str> {
    namespace
        .strip_suffix(TRANSLATED_SUFFIX)
        .and_then(|rest| rest.strip_suffix('/'))
        .filter(|broadcast| is_broadcast(broadcast))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_child_of_the_prefix_is_a_broadcast() {
        // Act / Assert
        assert!(is_broadcast("anon/live-translate/brave-otter"));
    }

    #[test]
    fn own_translated_namespace_is_not_a_broadcast() {
        // Act / Assert
        assert!(!is_broadcast("anon/live-translate/brave-otter/translated"));
    }

    #[test]
    fn prefix_itself_and_lookalike_prefixes_are_not_broadcasts() {
        // Act / Assert
        assert!(!is_broadcast("anon/live-translate"));
        assert!(!is_broadcast("anon/live-translate/"));
        assert!(!is_broadcast("anon/live-translate-old/brave-otter"));
    }

    #[test]
    fn translated_namespace_leads_back_to_its_broadcast() {
        // Act / Assert
        assert_eq!(
            broadcast_of_translated("anon/live-translate/brave-otter/translated"),
            Some("anon/live-translate/brave-otter")
        );
        assert_eq!(
            broadcast_of_translated("anon/live-translate/brave-otter"),
            None
        );
        assert_eq!(
            broadcast_of_translated("anon/other/brave-otter/translated"),
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
