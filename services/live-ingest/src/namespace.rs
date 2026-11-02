/// `anon/live/test` → `["anon", "live", "test"]`; empty elements are dropped,
/// so a leading, trailing or doubled `/` names the same namespace.
pub fn split_namespace(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_empty_elements() {
        // Act / Assert
        assert_eq!(
            split_namespace("/anon/live//test/"),
            ["anon", "live", "test"]
        );
        assert!(split_namespace("/").is_empty());
    }
}
