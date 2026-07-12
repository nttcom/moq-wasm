use std::net::{SocketAddr, ToSocketAddrs};
use std::str::FromStr;

use anyhow::{Context, Result, bail};

/// A validated relay endpoint: a host with a required port.
#[derive(Debug, Clone)]
pub struct RelayUrl {
    host: String,
    port: u16,
}

impl FromStr for RelayUrl {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let url = url::Url::parse(s).context("invalid relay URL")?;
        let host = url.host_str().context("relay URL has no host")?.to_string();
        let Some(port) = url.port() else {
            bail!("relay URL must include a port, got: {s}");
        };
        Ok(Self { host, port })
    }
}

impl RelayUrl {
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Resolve the host to a connectable address via DNS. The moqt QUIC client
    /// binds an IPv4 socket, so it can only reach IPv4 destinations; prefer
    /// IPv4 (e.g. localhost -> 127.0.0.1, not ::1).
    pub fn resolve(&self) -> Result<SocketAddr> {
        let addrs: Vec<SocketAddr> = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .context("failed to resolve relay address")?
            .collect();
        addrs
            .iter()
            .copied()
            .find(SocketAddr::is_ipv4)
            .or_else(|| addrs.first().copied())
            .context("relay address resolved to nothing")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_and_port() {
        let relay: RelayUrl = "moqt://localhost:4433".parse().unwrap();

        assert_eq!(relay.host, "localhost");
        assert_eq!(relay.port, 4433);
    }

    #[test]
    fn rejects_url_without_port() {
        assert!("moqt://localhost".parse::<RelayUrl>().is_err());
    }

    #[test]
    fn resolves_to_ipv4() {
        let relay: RelayUrl = "moqt://localhost:4433".parse().unwrap();

        assert!(relay.resolve().unwrap().is_ipv4());
    }
}
