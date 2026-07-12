use bytes::Bytes;
use packages::loc::{CaptureTimestamp, LocHeader, LocHeaderExtension};

/// Prefix marking a LOC header JSON blob inside a MOQ immutable extension (0xB).
/// Matches the browser publisher/receiver so the two interoperate.
const LOC_HEADER_SENTINEL: &[u8] = b"loc:";

/// Build the 0xB immutable-extension payload carrying the capture timestamp.
///
/// draft-ietf-moq-loc-01 §2.3.1.1 defines Capture Timestamp as header extension
/// ID 2 (a bare varint), but the moqt crate can only emit the 0x3c/0x3e/0xb
/// extension IDs, so we ride on 0xB with the `"loc:"+JSON` encoding the browser
/// uses. Spec-wise this is malformed, but it round-trips within this system.
pub fn capture_timestamp_ext(micros_since_unix_epoch: u64) -> Bytes {
    let header = LocHeader {
        extensions: vec![LocHeaderExtension::CaptureTimestamp(CaptureTimestamp {
            micros_since_unix_epoch,
        })],
    };
    let mut encoded = Vec::from(LOC_HEADER_SENTINEL);
    encoded.extend_from_slice(&serde_json::to_vec(&header).expect("LocHeader serializes"));
    Bytes::from(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_capture_timestamp_as_sentinel_prefixed_json() {
        let ext = capture_timestamp_ext(1_700_000_000_000_000);

        let json = ext
            .strip_prefix(LOC_HEADER_SENTINEL)
            .expect("sentinel prefix");
        assert_eq!(
            std::str::from_utf8(json).unwrap(),
            r#"{"extensions":[{"type":"captureTimestamp","value":{"microsSinceUnixEpoch":1700000000000000}}]}"#
        );
    }
}
