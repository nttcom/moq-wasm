#[cfg(not(target_arch = "wasm32"))]
use anyhow::Context;
#[cfg(not(target_arch = "wasm32"))]
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientTransport {
    Quic,
    WebTransport,
}

impl ClientTransport {
    fn from_scheme(scheme: &str) -> anyhow::Result<Self> {
        match scheme {
            "moqt" => Ok(Self::Quic),
            "https" => Ok(Self::WebTransport),
            scheme => anyhow::bail!(
                "unsupported connect url scheme {scheme:?}: expected moqt:// (QUIC) or https:// (WebTransport)"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectTarget {
    pub(crate) transport: ClientTransport,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) url: Url,
    #[cfg(target_arch = "wasm32")]
    pub(crate) url: String,
    #[cfg(not(target_arch = "wasm32"))]
    port: u16,
}

impl ConnectTarget {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn parse(url: &str) -> anyhow::Result<Self> {
        let url = Url::parse(url).with_context(|| format!("invalid connect url: {url}"))?;
        let transport = ClientTransport::from_scheme(url.scheme())?;
        if url.host().is_none() {
            anyhow::bail!("connect url has no host: {url}");
        }
        let port = url.port().unwrap_or(match transport {
            ClientTransport::Quic => 4433,
            ClientTransport::WebTransport => 443,
        });
        Ok(Self {
            transport,
            url,
            port,
        })
    }

    /// Host and port resolution is left to the browser's `WebTransport`.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn parse(url: &str) -> anyhow::Result<Self> {
        let parsed = web_sys::Url::new(url)
            .map_err(|error| anyhow::anyhow!("invalid connect url: {url}: {error:?}"))?;
        let transport = ClientTransport::from_scheme(parsed.protocol().trim_end_matches(':'))?;
        if parsed.hostname().is_empty() {
            anyhow::bail!("connect url has no host: {url}");
        }
        Ok(Self {
            transport,
            url: parsed.href(),
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn connect_quic(
        &self,
        endpoint: &quinn::Endpoint,
        config: quinn::ClientConfig,
    ) -> anyhow::Result<quinn::Connection> {
        let remote_address = self.resolve_remote_address().await?;
        let connection = endpoint
            .connect_with(config, remote_address, &self.server_name())?
            .await?;
        Ok(connection)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn server_name(&self) -> String {
        use url::Host;
        match self.host() {
            Host::Domain(domain) => domain.to_string(),
            Host::Ipv4(address) => address.to_string(),
            Host::Ipv6(address) => address.to_string(),
        }
    }

    /// The client endpoint binds an IPv4 socket, so an IPv4 address is
    /// preferred when the host resolves to both families.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn resolve_remote_address(&self) -> anyhow::Result<std::net::SocketAddr> {
        use std::net::{IpAddr, SocketAddr};
        use url::Host;

        let domain = match self.host() {
            Host::Domain(domain) => domain,
            Host::Ipv4(address) => return Ok(SocketAddr::new(IpAddr::V4(address), self.port)),
            Host::Ipv6(address) => return Ok(SocketAddr::new(IpAddr::V6(address), self.port)),
        };
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((domain, self.port))
            .await
            .with_context(|| format!("failed to resolve {domain}"))?
            .collect();
        addresses
            .iter()
            .copied()
            .find(SocketAddr::is_ipv4)
            .or_else(|| addresses.first().copied())
            .with_context(|| format!("{domain} resolved to no address"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn host(&self) -> url::Host<&str> {
        self.url
            .host()
            .expect("ConnectTarget::parse rejects urls without a host")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moqt_scheme_selects_quic_with_default_port() {
        // Act
        let target = ConnectTarget::parse("moqt://relay.example").unwrap();

        // Assert
        assert_eq!(target.transport, ClientTransport::Quic);
        assert_eq!(target.port, 4433);
        assert_eq!(target.server_name(), "relay.example");
    }

    #[test]
    fn https_scheme_selects_web_transport_with_default_port() {
        // Act
        let target = ConnectTarget::parse("https://relay.example/moq").unwrap();

        // Assert
        assert_eq!(target.transport, ClientTransport::WebTransport);
        assert_eq!(target.port, 443);
        assert_eq!(target.url.path(), "/moq");
    }

    #[test]
    fn explicit_port_overrides_default() {
        // Act
        let target = ConnectTarget::parse("moqt://relay.example:9000").unwrap();

        // Assert
        assert_eq!(target.port, 9000);
    }

    #[test]
    fn rejects_unknown_scheme() {
        // Act / Assert
        assert!(ConnectTarget::parse("http://relay.example").is_err());
    }

    #[test]
    fn rejects_url_without_host() {
        // Act / Assert
        assert!(ConnectTarget::parse("moqt:///path").is_err());
    }

    #[tokio::test]
    async fn ip_literal_hosts_resolve_without_dns() {
        // Arrange
        let ipv4 = ConnectTarget::parse("moqt://127.0.0.1:4433").unwrap();
        let ipv6 = ConnectTarget::parse("moqt://[::1]:4433").unwrap();

        // Act
        let ipv4_address = ipv4.resolve_remote_address().await.unwrap();
        let ipv6_address = ipv6.resolve_remote_address().await.unwrap();

        // Assert
        assert_eq!(ipv4_address, "127.0.0.1:4433".parse().unwrap());
        assert_eq!(ipv6_address, "[::1]:4433".parse().unwrap());
        assert_eq!(ipv6.server_name(), "::1");
    }
}
