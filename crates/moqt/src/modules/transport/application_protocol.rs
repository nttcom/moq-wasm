//! The application protocol a connection negotiates: the ALPN of a raw QUIC
//! connection, or the WebTransport subprotocol (WT-Available-Protocols /
//! WT-Protocol) of a WebTransport session.

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
    fn no_subprotocol_is_selected_when_none_is_shared() {
        // Act
        let selected = PROTOCOLS.select_web_transport(&["moqt-c".into()]);

        // Assert
        assert_eq!(selected, None);
    }
}
