//! The WebSocket subprotocol (Sec-WebSocket-Protocol) of a QMux session.
//! draft-lcurley-qmux-websocket-00 §5 carries the application ALPN; this is
//! the form moq-dev's qmux implementations use, `qmux-02.<alpn>`, with the
//! bare `qmux-02` for a session without an application protocol.

const QMUX_VERSION: &str = "qmux-02";

/// The subprotocol that pairs QMux draft-02 with `alpn`.
pub(crate) fn subprotocol_for(alpn: Option<&str>) -> String {
    match alpn {
        Some(alpn) => format!("{QMUX_VERSION}.{alpn}"),
        None => QMUX_VERSION.to_string(),
    }
}

/// The application ALPN a subprotocol names; `None` when it is not QMux
/// draft-02, `Some(None)` for the bare form.
pub(crate) fn alpn_of(subprotocol: &str) -> Option<Option<&str>> {
    if subprotocol == QMUX_VERSION {
        return Some(None);
    }
    subprotocol
        .strip_prefix(QMUX_VERSION)
        .and_then(|rest| rest.strip_prefix('.'))
        .filter(|alpn| !alpn.is_empty())
        .map(Some)
}

/// The subprotocols a Sec-WebSocket-Protocol header value offers.
pub(crate) fn offered(header: &str) -> Vec<String> {
    header
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subprotocol_names_the_qmux_draft_and_the_alpn() {
        // Act / Assert
        assert_eq!(subprotocol_for(Some("moq-00")), "qmux-02.moq-00");
        assert_eq!(subprotocol_for(None), "qmux-02");
    }

    #[test]
    fn the_alpn_is_read_back_from_both_forms() {
        // Act / Assert
        assert_eq!(alpn_of("qmux-02.moq-00"), Some(Some("moq-00")));
        assert_eq!(alpn_of("qmux-02"), Some(None));
        assert_eq!(alpn_of("qmux-02."), None);
        assert_eq!(alpn_of("qmux-01.moq-00"), None);
        assert_eq!(alpn_of("moq-00"), None);
    }

    #[test]
    fn a_header_value_lists_subprotocols_in_offer_order() {
        // Act
        let offered = offered("qmux-02.moq-00, qmux-02 ,,qmux-01");

        // Assert
        assert_eq!(offered, ["qmux-02.moq-00", "qmux-02", "qmux-01"]);
    }
}
