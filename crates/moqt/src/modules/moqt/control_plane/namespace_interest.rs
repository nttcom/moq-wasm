/// What a namespace subscription asks the peer for. draft-18 splits it in
/// two requests, SUBSCRIBE_NAMESPACE for namespaces and SUBSCRIBE_TRACKS for
/// tracks (draft-18 §6.1); draft-14 has only SUBSCRIBE_NAMESPACE, which always
/// covers both (draft-14 §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamespaceInterest {
    /// The namespaces published under the prefix, as PUBLISH_NAMESPACE /
    /// PUBLISH_NAMESPACE_DONE events.
    Namespaces,
    /// The tracks published under the prefix, as PUBLISH requests.
    Tracks,
    NamespacesAndTracks,
}

impl NamespaceInterest {
    pub fn includes_namespaces(self) -> bool {
        matches!(self, Self::Namespaces | Self::NamespacesAndTracks)
    }

    pub fn includes_tracks(self) -> bool {
        matches!(self, Self::Tracks | Self::NamespacesAndTracks)
    }

    /// What `self` and `other` ask for together.
    pub(crate) fn with(self, other: Self) -> Self {
        match (
            self.includes_namespaces() || other.includes_namespaces(),
            self.includes_tracks() || other.includes_tracks(),
        ) {
            (true, true) => Self::NamespacesAndTracks,
            (true, false) => Self::Namespaces,
            (false, _) => Self::Tracks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NamespaceInterest;

    #[test]
    fn namespaces_with_tracks_ask_for_both() {
        // Act / Assert
        assert_eq!(
            NamespaceInterest::Namespaces.with(NamespaceInterest::Tracks),
            NamespaceInterest::NamespacesAndTracks
        );
        assert_eq!(
            NamespaceInterest::Tracks.with(NamespaceInterest::Tracks),
            NamespaceInterest::Tracks
        );
    }
}
