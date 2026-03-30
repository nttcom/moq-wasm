#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RelayTransport {
    Stream,
    Datagram,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CacheLocation {
    pub(crate) group_id: u64,
    pub(crate) index: u64,
}