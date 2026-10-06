use std::net::{IpAddr, SocketAddr};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransportAddresses {
    pub remote: Option<SocketAddr>,
    pub local_ip: Option<IpAddr>,
}

#[cfg(not(target_arch = "wasm32"))]
impl From<&quinn::Connection> for TransportAddresses {
    fn from(connection: &quinn::Connection) -> Self {
        Self {
            remote: Some(connection.remote_address()),
            local_ip: connection.local_ip(),
        }
    }
}
