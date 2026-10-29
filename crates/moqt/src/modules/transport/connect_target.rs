#[cfg(not(target_arch = "wasm32"))]
use anyhow::Context;
#[cfg(not(target_arch = "wasm32"))]
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientTransport {
    Quic,
    WebTransport,
    /// QMux over WebSocket.
    WebSocket,
}

impl ClientTransport {
    fn from_scheme(scheme: &str) -> anyhow::Result<Self> {
        match scheme {
            "moqt" => Ok(Self::Quic),
            "https" => Ok(Self::WebTransport),
            "wss" => Ok(Self::WebSocket),
            scheme => anyhow::bail!(
                "unsupported connect url scheme {scheme:?}: expected moqt:// (QUIC), https:// (WebTransport) or wss:// (WebSocket)"
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
    /// Set by `resolved`, so that an attempt and its fallback go to one address.
    #[cfg(not(target_arch = "wasm32"))]
    resolved: Option<std::net::SocketAddr>,
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
            ClientTransport::WebTransport | ClientTransport::WebSocket => 443,
        });
        Ok(Self {
            transport,
            url,
            port,
            resolved: None,
        })
    }

    /// Resolves the host now, so DNS time does not count against the fallback wait.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn resolved(mut self) -> anyhow::Result<Self> {
        self.resolved = Some(self.resolve_remote_address().await?);
        Ok(self)
    }

    /// The browser resolves the host itself.
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn resolved(self) -> anyhow::Result<Self> {
        Ok(self)
    }

    /// The `wss://` target on the same host and port; `None` for a `wss://`
    /// target. `moqt://` has no path, and the server ignores the WebSocket
    /// path, so it gets `/`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn web_socket_fallback(&self) -> Option<Self> {
        let rest = match self.transport {
            ClientTransport::Quic => "/".to_string(),
            ClientTransport::WebTransport => match self.url.query() {
                Some(query) => format!("{}?{query}", self.url.path()),
                None => self.url.path().to_string(),
            },
            ClientTransport::WebSocket => return None,
        };
        let host = self
            .url
            .host_str()
            .expect("ConnectTarget::parse rejects urls without a host");
        let mut web_socket = Self::parse(&format!("wss://{host}:{}{rest}", self.port)).ok()?;
        web_socket.resolved = self.resolved;
        Some(web_socket)
    }

    /// The host and port a fallback is remembered for.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn authority(&self) -> String {
        let host = self
            .url
            .host_str()
            .expect("ConnectTarget::parse rejects urls without a host");
        format!("{host}:{}", self.port)
    }

    /// The browser cannot open raw QUIC, so only `https://` falls back.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn web_socket_fallback(&self) -> Option<Self> {
        if self.transport != ClientTransport::WebTransport {
            return None;
        }
        let url = web_sys::Url::new(&self.url).ok()?;
        url.set_protocol("wss:");
        Self::parse(&url.href()).ok()
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn authority(&self) -> String {
        web_sys::Url::new(&self.url)
            .map(|url| url.host())
            .unwrap_or_default()
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

        if let Some(address) = self.resolved {
            return Ok(address);
        }

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
    fn wss_scheme_selects_web_socket_with_default_port() {
        // Act
        let target = ConnectTarget::parse("wss://relay.example/moq").unwrap();

        // Assert
        assert_eq!(target.transport, ClientTransport::WebSocket);
        assert_eq!(target.port, 443);
    }

    #[test]
    fn a_failed_target_falls_back_to_wss_on_the_same_host_and_port() {
        // Arrange
        let quic = ConnectTarget::parse("moqt://relay.example").unwrap();
        let web_transport = ConnectTarget::parse("https://relay.example/moq?room=1").unwrap();
        let ipv6 = ConnectTarget::parse("https://[::1]:4433/moq").unwrap();
        let web_socket = ConnectTarget::parse("wss://relay.example/moq").unwrap();

        // Act / Assert: moqt:// keeps its 4433 although wss:// defaults to 443
        assert_eq!(
            quic.web_socket_fallback().unwrap().url.as_str(),
            "wss://relay.example:4433/"
        );
        assert_eq!(
            web_transport.web_socket_fallback().unwrap().url.as_str(),
            "wss://relay.example/moq?room=1"
        );
        assert_eq!(
            ipv6.web_socket_fallback().unwrap().url.as_str(),
            "wss://[::1]:4433/moq"
        );
        assert!(web_socket.web_socket_fallback().is_none());
        assert_eq!(quic.authority(), "relay.example:4433");
    }

    #[test]
    fn the_fallback_of_a_resolved_target_goes_to_the_same_address() {
        // Arrange: an address DNS would not return for the name
        let mut target = ConnectTarget::parse("moqt://localhost:4433").unwrap();
        target.resolved = Some("127.0.0.2:4433".parse().unwrap());

        // Act
        let fallback = target.web_socket_fallback().unwrap();

        // Assert
        assert_eq!(fallback.resolved, target.resolved);
        assert_eq!(fallback.server_name(), "localhost");
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
