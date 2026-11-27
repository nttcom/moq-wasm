use mediapack::loc::{
    LocExtension, LocObject, TIMESTAMP_ID, from_extension_headers, to_extension_headers,
};
use moqt::object::ExtensionHeaders;

/// The LOC Timestamp property of an object's extension headers as wall-clock
/// µs since the Unix epoch (draft-ietf-moq-loc-04 §2.3.1.1). `None` without a
/// Timestamp, or when a Timescale property makes it media time with an
/// application-defined epoch (§2.3.1.2).
pub fn loc_timestamp_micros(headers: &ExtensionHeaders) -> Option<u64> {
    let object = LocObject {
        extensions: from_extension_headers(headers),
        ..LocObject::default()
    };
    object
        .capture_timestamp()
        .map(|timestamp| timestamp.micros())
}

/// Extension headers carrying only a LOC Timestamp property of wall-clock µs
/// since the Unix epoch (draft-ietf-moq-loc-04 §2.3.1.1), for
/// `TrackWriter::write`.
pub fn loc_timestamp_headers(timestamp_micros: u64) -> ExtensionHeaders {
    to_extension_headers(&[LocExtension::varint(TIMESTAMP_ID, timestamp_micros)])
}

#[cfg(test)]
mod tests {
    use mediapack::loc::TIMESCALE_ID;

    use super::*;

    #[test]
    fn reads_back_the_timestamp_it_writes() {
        // Arrange
        let headers = loc_timestamp_headers(1_700_000_000_000_000);

        // Act
        let micros = loc_timestamp_micros(&headers);

        // Assert
        assert_eq!(micros, Some(1_700_000_000_000_000));
    }

    #[test]
    fn reports_no_timestamp_without_the_property() {
        // Act / Assert
        assert_eq!(loc_timestamp_micros(&ExtensionHeaders::default()), None);
    }

    #[test]
    fn reports_no_timestamp_for_media_time() {
        // Arrange
        let headers = to_extension_headers(&[
            LocExtension::varint(TIMESCALE_ID, 48_000),
            LocExtension::varint(TIMESTAMP_ID, 96_000),
        ]);

        // Act / Assert
        assert_eq!(loc_timestamp_micros(&headers), None);
    }
}
