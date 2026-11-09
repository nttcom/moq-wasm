use bytes::Bytes;

use crate::modules::moqt::control_plane::control_messages::key_value_pair::{
    KeyValuePair, odd_values,
};

/// The TRACE_CONTEXT parameter: the W3C `traceparent` of the span that sent
/// the request, so the receiver's span for it joins the sender's trace.
///
/// Defined by neither draft-14 nor draft-18. Its type is a private odd
/// number, so the value is length-prefixed bytes. Draft-14 §9.2 makes a
/// receiver ignore a parameter it does not recognize, but in draft-18 an
/// unknown Message Parameter is a PROTOCOL_VIOLATION (§10.2) and an
/// extension is agreed through a Setup Option (§3.2), so on both drafts it
/// is sent only to a peer whose SETUP carried the TRACE_CONTEXT Setup Option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext(String);

impl TraceContext {
    const PARAMETER_TYPE: u64 = 0x1fff;

    /// `None` unless `traceparent` is `00-<32 hex>-<16 hex>-<2 hex>`.
    pub fn parse(traceparent: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(traceparent).ok()?;
        let mut fields = text.split('-');
        let well_formed = [2, 32, 16, 2].iter().all(|&length| {
            fields.next().is_some_and(|field| {
                field.len() == length
                    && field
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            })
        }) && fields.next().is_none();
        well_formed.then(|| Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first TRACE_CONTEXT in `parameters`. A value that is not a
    /// traceparent counts as absent: its Key-Value serialization (length and
    /// bytes) is valid, so KEY_VALUE_FORMATTING_ERROR (draft-14 §1.4.2) does
    /// not apply.
    pub(crate) fn find(parameters: &[KeyValuePair]) -> Option<Self> {
        odd_values(parameters, Self::PARAMETER_TYPE)
            .next()
            .and_then(|value| Self::parse(value))
    }

    pub(crate) fn encode(&self) -> KeyValuePair {
        KeyValuePair::odd(
            Self::PARAMETER_TYPE,
            Bytes::copy_from_slice(self.0.as_bytes()),
        )
    }
}

#[cfg(test)]
pub(crate) const TEST_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

#[cfg(test)]
mod tests {
    use super::{TEST_TRACEPARENT, TraceContext};
    use crate::modules::moqt::control_plane::control_messages::key_value_pair::KeyValuePair;
    use bytes::Bytes;

    #[test]
    fn a_w3c_traceparent_parses() {
        // Act
        let trace_context = TraceContext::parse(TEST_TRACEPARENT.as_bytes());

        // Assert
        assert_eq!(
            trace_context.as_ref().map(TraceContext::as_str),
            Some(TEST_TRACEPARENT)
        );
    }

    #[test]
    fn a_malformed_traceparent_is_rejected() {
        // Arrange
        let malformed = [
            "",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-00",
        ];

        // Act / Assert
        for value in malformed {
            assert_eq!(TraceContext::parse(value.as_bytes()), None, "{value}");
        }
    }

    #[test]
    fn an_unparsable_parameter_counts_as_absent() {
        // Arrange
        let parameters = [KeyValuePair::odd(0x1fff, Bytes::from_static(b"garbage"))];

        // Act / Assert
        assert_eq!(TraceContext::find(&parameters), None);
    }
}
