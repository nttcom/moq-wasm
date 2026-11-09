#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionPeer {
    Client,
    Relay,
}

impl SessionPeer {
    pub(crate) fn kind(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Relay => "relay",
        }
    }
}
