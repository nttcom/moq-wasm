//! The application protocol a connection negotiates: the ALPN of a raw QUIC
//! connection, the WebTransport subprotocol (WT-Available-Protocols /
//! WT-Protocol) of a WebTransport session, or the WebSocket subprotocol
//! (Sec-WebSocket-Protocol) of a QMux session.

#[cfg(not(target_arch = "wasm32"))]
use crate::modules::transport::qmux::subprotocol;

/// What a client offers and a server accepts, most preferred first.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ApplicationProtocols {
    // wasm32 has no raw QUIC transport.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) quic_alpns: &'static [&'static str],
    pub(crate) web_transport: &'static [&'static str],
}

impl ApplicationProtocols {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn quic_alpn_list(&self) -> Vec<Vec<u8>> {
        self.quic_alpns
            .iter()
            .map(|alpn| alpn.as_bytes().to_vec())
            .collect()
    }

    /// The first of the server's subprotocols the client also offered.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn select_web_transport(&self, offered: &[String]) -> Option<&'static str> {
        self.web_transport
            .iter()
            .copied()
            .find(|protocol| offered.iter().any(|offered| offered == protocol))
    }

    /// The WebSocket subprotocols, most preferred first: QMux with each ALPN,
    /// then QMux without an application protocol.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn web_socket_subprotocols(&self) -> Vec<String> {
        self.quic_alpns
            .iter()
            .map(|alpn| subprotocol::subprotocol_for(Some(alpn)))
            .chain(std::iter::once(subprotocol::subprotocol_for(None)))
            .collect()
    }

    /// The first of the server's WebSocket subprotocols the client also offered.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn select_web_socket(&self, offered: &[String]) -> Option<String> {
        self.web_socket_subprotocols()
            .into_iter()
            .find(|protocol| offered.contains(protocol))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NegotiatedProtocol {
    // wasm32 has no raw QUIC transport.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Quic {
        alpn: String,
    },
    WebTransport {
        protocol: Option<String>,
    },
    /// QMux over WebSocket; `alpn` is what the subprotocol named.
    // Nothing on wasm32 constructs this variant.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    WebSocket {
        alpn: Option<String>,
    },
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::ApplicationProtocols;

    const PROTOCOLS: ApplicationProtocols = ApplicationProtocols {
        quic_alpns: &[],
        web_transport: &["moqt-b", "moqt-a"],
    };

    #[test]
    fn the_server_preference_decides_among_offered_subprotocols() {
        // Act
        let selected = PROTOCOLS.select_web_transport(&["moqt-a".into(), "moqt-b".into()]);

        // Assert
        assert_eq!(selected, Some("moqt-b"));
    }

    #[test]
    fn web_socket_subprotocols_pair_qmux_with_each_alpn_and_prefer_the_alpn_form() {
        // Arrange
        let protocols = ApplicationProtocols {
            quic_alpns: &["moq-00"],
            web_transport: &[],
        };

        // Act
        let offered = protocols.web_socket_subprotocols();
        let selected = protocols.select_web_socket(&["qmux-02".into(), "qmux-02.moq-00".into()]);
        let none = protocols.select_web_socket(&["qmux-01.moq-00".into()]);

        // Assert
        assert_eq!(offered, ["qmux-02.moq-00", "qmux-02"]);
        assert_eq!(selected.as_deref(), Some("qmux-02.moq-00"));
        assert_eq!(none, None);
    }

    #[test]
    fn no_subprotocol_is_selected_when_none_is_shared() {
        // Act
        let selected = PROTOCOLS.select_web_transport(&["moqt-c".into()]);

        // Assert
        assert_eq!(selected, None);
    }
}
