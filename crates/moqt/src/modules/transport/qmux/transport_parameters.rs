//! The transport parameters a QX_TRANSPORT_PARAMETERS frame carries
//! (draft-ietf-quic-qmux-02 §5), encoded as in RFC 9000 §18.

use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};

use super::frame::{ProtocolError, TransportErrorCode};
use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt};

const MAX_IDLE_TIMEOUT: u64 = 0x01;
const INITIAL_MAX_DATA: u64 = 0x04;
const INITIAL_MAX_STREAM_DATA_BIDI_LOCAL: u64 = 0x05;
const INITIAL_MAX_STREAM_DATA_BIDI_REMOTE: u64 = 0x06;
const INITIAL_MAX_STREAM_DATA_UNI: u64 = 0x07;
const INITIAL_MAX_STREAMS_BIDI: u64 = 0x08;
const INITIAL_MAX_STREAMS_UNI: u64 = 0x09;
/// RFC 9221 §3.
const MAX_DATAGRAM_FRAME_SIZE: u64 = 0x20;
/// §5.2.
const MAX_RECORD_SIZE: u64 = 0x0571c59429cd0845;

/// §5.1: the parameters about connection ids, UDP, acknowledgements and
/// migration, which a QMux endpoint must reject.
const FORBIDDEN: &[u64] = &[0x00, 0x02, 0x03, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10];

/// §5.2: with the 2-byte Size field a 16 KB record, a full-sized TLS record.
pub(crate) const DEFAULT_MAX_RECORD_SIZE: u64 = 16382;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransportParameters {
    /// `None` disables the idle timeout (RFC 9000 §10.1).
    pub(crate) max_idle_timeout: Option<Duration>,
    pub(crate) initial_max_data: u64,
    pub(crate) initial_max_stream_data_bidi_local: u64,
    pub(crate) initial_max_stream_data_bidi_remote: u64,
    pub(crate) initial_max_stream_data_uni: u64,
    pub(crate) initial_max_streams_bidi: u64,
    pub(crate) initial_max_streams_uni: u64,
    /// `None` when the peer accepts no DATAGRAM frames (RFC 9221 §3).
    pub(crate) max_datagram_frame_size: Option<u64>,
    pub(crate) max_record_size: u64,
}

/// RFC 9000 §18.2: every limit is 0 and the idle timeout is disabled when
/// a parameter is absent.
impl Default for TransportParameters {
    fn default() -> Self {
        Self {
            max_idle_timeout: None,
            initial_max_data: 0,
            initial_max_stream_data_bidi_local: 0,
            initial_max_stream_data_bidi_remote: 0,
            initial_max_stream_data_uni: 0,
            initial_max_streams_bidi: 0,
            initial_max_streams_uni: 0,
            max_datagram_frame_size: None,
            max_record_size: DEFAULT_MAX_RECORD_SIZE,
        }
    }
}

impl TransportParameters {
    pub(crate) fn encode(&self) -> Bytes {
        let mut out = BytesMut::new();
        if let Some(timeout) = self.max_idle_timeout {
            put_integer(&mut out, MAX_IDLE_TIMEOUT, timeout.as_millis() as u64);
        }
        put_integer(&mut out, INITIAL_MAX_DATA, self.initial_max_data);
        put_integer(
            &mut out,
            INITIAL_MAX_STREAM_DATA_BIDI_LOCAL,
            self.initial_max_stream_data_bidi_local,
        );
        put_integer(
            &mut out,
            INITIAL_MAX_STREAM_DATA_BIDI_REMOTE,
            self.initial_max_stream_data_bidi_remote,
        );
        put_integer(
            &mut out,
            INITIAL_MAX_STREAM_DATA_UNI,
            self.initial_max_stream_data_uni,
        );
        put_integer(
            &mut out,
            INITIAL_MAX_STREAMS_BIDI,
            self.initial_max_streams_bidi,
        );
        put_integer(
            &mut out,
            INITIAL_MAX_STREAMS_UNI,
            self.initial_max_streams_uni,
        );
        if let Some(size) = self.max_datagram_frame_size {
            put_integer(&mut out, MAX_DATAGRAM_FRAME_SIZE, size);
        }
        if self.max_record_size != DEFAULT_MAX_RECORD_SIZE {
            put_integer(&mut out, MAX_RECORD_SIZE, self.max_record_size);
        }
        out.freeze()
    }

    pub(crate) fn decode(buf: &mut Bytes) -> Result<Self, ProtocolError> {
        let mut parameters = Self::default();
        let mut seen = Vec::new();
        while buf.has_remaining() {
            let id = get_varint(buf)?;
            let length = get_varint(buf)? as usize;
            if buf.remaining() < length {
                return Err(error("a transport parameter is truncated"));
            }
            let mut value = buf.split_to(length);
            if FORBIDDEN.contains(&id) {
                return Err(error(format!("forbidden transport parameter {id:#x}")));
            }
            if seen.contains(&id) {
                return Err(error(format!("transport parameter {id:#x} sent twice")));
            }
            seen.push(id);
            match id {
                MAX_IDLE_TIMEOUT => {
                    let millis = get_integer(&mut value)?;
                    parameters.max_idle_timeout =
                        (millis > 0).then(|| Duration::from_millis(millis));
                }
                INITIAL_MAX_DATA => parameters.initial_max_data = get_integer(&mut value)?,
                INITIAL_MAX_STREAM_DATA_BIDI_LOCAL => {
                    parameters.initial_max_stream_data_bidi_local = get_integer(&mut value)?
                }
                INITIAL_MAX_STREAM_DATA_BIDI_REMOTE => {
                    parameters.initial_max_stream_data_bidi_remote = get_integer(&mut value)?
                }
                INITIAL_MAX_STREAM_DATA_UNI => {
                    parameters.initial_max_stream_data_uni = get_integer(&mut value)?
                }
                INITIAL_MAX_STREAMS_BIDI => {
                    parameters.initial_max_streams_bidi = get_integer(&mut value)?
                }
                INITIAL_MAX_STREAMS_UNI => {
                    parameters.initial_max_streams_uni = get_integer(&mut value)?
                }
                MAX_DATAGRAM_FRAME_SIZE => {
                    let size = get_integer(&mut value)?;
                    parameters.max_datagram_frame_size = (size > 0).then_some(size);
                }
                MAX_RECORD_SIZE => {
                    let size = get_integer(&mut value)?;
                    if size < DEFAULT_MAX_RECORD_SIZE {
                        return Err(error(format!(
                            "max_record_size {size} is below the default {DEFAULT_MAX_RECORD_SIZE}"
                        )));
                    }
                    parameters.max_record_size = size;
                }
                // RFC 9000 §7.4.2: unknown parameters, including the reserved
                // ones, are ignored.
                _ => {}
            }
        }
        Ok(parameters)
    }
}

fn put_integer(out: &mut BytesMut, id: u64, value: u64) {
    let mut encoded = BytesMut::new();
    encoded.put_varint(value);
    out.put_varint(id);
    out.put_varint(encoded.len() as u64);
    out.extend_from_slice(&encoded);
}

fn get_integer(value: &mut Bytes) -> Result<u64, ProtocolError> {
    let integer = get_varint(value)?;
    if value.has_remaining() {
        return Err(error("an integer transport parameter has trailing bytes"));
    }
    Ok(integer)
}

fn get_varint(buf: &mut Bytes) -> Result<u64, ProtocolError> {
    buf.try_get_varint()
        .map_err(|_| error("a transport parameter is truncated"))
}

fn error(reason: impl Into<String>) -> ProtocolError {
    ProtocolError::new(TransportErrorCode::TransportParameterError, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TransportParameters {
        TransportParameters {
            max_idle_timeout: Some(Duration::from_secs(30)),
            initial_max_data: 16 << 20,
            initial_max_stream_data_bidi_local: 1 << 20,
            initial_max_stream_data_bidi_remote: 2 << 20,
            initial_max_stream_data_uni: 3 << 20,
            initial_max_streams_bidi: 100,
            initial_max_streams_uni: 1000,
            max_datagram_frame_size: Some(16382),
            max_record_size: 32_768,
        }
    }

    #[test]
    fn parameters_survive_a_round_trip() {
        // Act
        let decoded = TransportParameters::decode(&mut sample().encode()).unwrap();

        // Assert
        assert_eq!(decoded, sample());
    }

    #[test]
    fn absent_parameters_take_their_defaults() {
        // Act
        let decoded = TransportParameters::decode(&mut Bytes::new()).unwrap();

        // Assert
        assert_eq!(decoded, TransportParameters::default());
        assert_eq!(decoded.max_record_size, DEFAULT_MAX_RECORD_SIZE);
    }

    #[test]
    fn unknown_parameters_are_ignored() {
        // Arrange: a reserved id (31 * 2 + 27 = 89) with an opaque value
        let mut encoded = BytesMut::new();
        encoded.put_varint(89);
        encoded.put_varint(3);
        encoded.extend_from_slice(b"xyz");

        // Act
        let decoded = TransportParameters::decode(&mut encoded.freeze()).unwrap();

        // Assert
        assert_eq!(decoded, TransportParameters::default());
    }

    #[test]
    fn forbidden_duplicate_and_small_record_size_parameters_are_errors() {
        // Arrange
        let mut forbidden = BytesMut::new();
        put_integer(&mut forbidden, 0x03, 1200);
        let mut duplicate = BytesMut::new();
        put_integer(&mut duplicate, INITIAL_MAX_DATA, 1);
        put_integer(&mut duplicate, INITIAL_MAX_DATA, 2);
        let mut small = BytesMut::new();
        put_integer(&mut small, MAX_RECORD_SIZE, 1024);

        // Act / Assert
        for mut encoded in [forbidden.freeze(), duplicate.freeze(), small.freeze()] {
            let error = TransportParameters::decode(&mut encoded).unwrap_err();
            assert_eq!(error.code, TransportErrorCode::TransportParameterError);
        }
    }
}
