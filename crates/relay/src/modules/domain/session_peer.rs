#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionPeer {
    Client,
    Relay { relay_id: Option<String> },
}

impl SessionPeer {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Relay { .. } => "relay",
        }
    }

    pub(crate) fn relay_id(&self) -> Option<&str> {
        match self {
            Self::Client => None,
            Self::Relay { relay_id } => relay_id.as_deref(),
        }
    }
}
