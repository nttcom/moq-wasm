pub(crate) type TrackNamespace = String;
pub(crate) type TrackNamespacePrefix = String;

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
