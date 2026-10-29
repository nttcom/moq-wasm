//! The frames a QMux record carries (draft-ietf-quic-qmux-02 §4): the QUIC
//! version 1 frames QMux permits, the DATAGRAM frame of RFC 9221 (§9.1) and
//! the two QX frames QMux adds.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

use super::transport_parameters::TransportParameters;
use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt};

/// The QUIC transport error codes (RFC 9000 §20.1) a QMux endpoint closes
/// the connection with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportErrorCode {
    FlowControlError = 0x03,
    StreamLimitError = 0x04,
    StreamStateError = 0x05,
    FinalSizeError = 0x06,
    FrameEncodingError = 0x07,
    TransportParameterError = 0x08,
    ProtocolViolation = 0x0a,
}

/// A violation that closes the connection with CONNECTION_CLOSE 0x1c.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code:?}: {reason}")]
pub(crate) struct ProtocolError {
    pub(crate) code: TransportErrorCode,
    pub(crate) reason: String,
}

impl ProtocolError {
    pub(crate) fn new(code: TransportErrorCode, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }
}

const RESET_STREAM: u64 = 0x04;
const STOP_SENDING: u64 = 0x05;
const STREAM_BASE: u64 = 0x08;
const STREAM_FIN_BIT: u64 = 0x01;
const STREAM_LEN_BIT: u64 = 0x02;
const STREAM_OFF_BIT: u64 = 0x04;
const MAX_DATA: u64 = 0x10;
const MAX_STREAM_DATA: u64 = 0x11;
const MAX_STREAMS_BIDI: u64 = 0x12;
const MAX_STREAMS_UNI: u64 = 0x13;
const DATA_BLOCKED: u64 = 0x14;
const STREAM_DATA_BLOCKED: u64 = 0x15;
const STREAMS_BLOCKED_BIDI: u64 = 0x16;
const STREAMS_BLOCKED_UNI: u64 = 0x17;
const CONNECTION_CLOSE_TRANSPORT: u64 = 0x1c;
const CONNECTION_CLOSE_APPLICATION: u64 = 0x1d;
const DATAGRAM: u64 = 0x30;
const DATAGRAM_WITH_LENGTH: u64 = 0x31;
/// §4.2; "\xffQS0\r\n\r\n" on the wire, distinct from any HTTP/1.1 or HTTP/2 start.
const QX_TRANSPORT_PARAMETERS: u64 = 0x3f5153300d0a0d0a;
const QX_PING: u64 = 0x348c67529ef8c7bd;
const QX_PING_RESPONSE: u64 = 0x348c67529ef8c7be;

/// The frames QMux prohibits (§4): the cryptographic handshake, connection ids,
/// path validation and acknowledgements are the underlying transport's job.
const PROHIBITED: &[u64] = &[0x01, 0x02, 0x03, 0x06, 0x07, 0x18, 0x19, 0x1a, 0x1b, 0x1e];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionClose {
    /// 0x1c: a transport error, `error_code` being a `TransportErrorCode`.
    Transport {
        error_code: u64,
        frame_type: u64,
        reason: String,
    },
    /// 0x1d: the application closed, `error_code` being its own code.
    Application { error_code: u64, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    ResetStream {
        stream_id: u64,
        error_code: u64,
        final_size: u64,
    },
    StopSending {
        stream_id: u64,
        error_code: u64,
    },
    Stream {
        stream_id: u64,
        offset: u64,
        data: Bytes,
        fin: bool,
    },
    MaxData {
        maximum: u64,
    },
    MaxStreamData {
        stream_id: u64,
        maximum: u64,
    },
    MaxStreams {
        bidirectional: bool,
        maximum: u64,
    },
    DataBlocked {
        limit: u64,
    },
    StreamDataBlocked {
        stream_id: u64,
        limit: u64,
    },
    StreamsBlocked {
        bidirectional: bool,
        limit: u64,
    },
    ConnectionClose(ConnectionClose),
    Datagram {
        data: Bytes,
    },
    TransportParameters(TransportParameters),
    Ping {
        sequence: u64,
    },
    PingResponse {
        sequence: u64,
    },
}

impl Frame {
    /// PADDING (type 0) is skipped.
    pub(crate) fn decode_record(mut record: Bytes) -> Result<Vec<Frame>, ProtocolError> {
        let mut frames = Vec::new();
        while record.has_remaining() {
            if record[0] == 0 {
                record.advance(1);
                continue;
            }
            frames.push(Self::decode(&mut record)?);
        }
        Ok(frames)
    }

    fn decode(buf: &mut Bytes) -> Result<Frame, ProtocolError> {
        let frame_type = get_varint(buf)?;
        let frame = match frame_type {
            RESET_STREAM => Frame::ResetStream {
                stream_id: get_varint(buf)?,
                error_code: get_varint(buf)?,
                final_size: get_varint(buf)?,
            },
            STOP_SENDING => Frame::StopSending {
                stream_id: get_varint(buf)?,
                error_code: get_varint(buf)?,
            },
            STREAM_BASE..=0x0f => {
                let stream_id = get_varint(buf)?;
                let offset = if frame_type & STREAM_OFF_BIT != 0 {
                    get_varint(buf)?
                } else {
                    0
                };
                let data = if frame_type & STREAM_LEN_BIT != 0 {
                    get_bytes(buf)?
                } else {
                    buf.split_to(buf.remaining())
                };
                Frame::Stream {
                    stream_id,
                    offset,
                    data,
                    fin: frame_type & STREAM_FIN_BIT != 0,
                }
            }
            MAX_DATA => Frame::MaxData {
                maximum: get_varint(buf)?,
            },
            MAX_STREAM_DATA => Frame::MaxStreamData {
                stream_id: get_varint(buf)?,
                maximum: get_varint(buf)?,
            },
            MAX_STREAMS_BIDI | MAX_STREAMS_UNI => Frame::MaxStreams {
                bidirectional: frame_type == MAX_STREAMS_BIDI,
                maximum: get_varint(buf)?,
            },
            DATA_BLOCKED => Frame::DataBlocked {
                limit: get_varint(buf)?,
            },
            STREAM_DATA_BLOCKED => Frame::StreamDataBlocked {
                stream_id: get_varint(buf)?,
                limit: get_varint(buf)?,
            },
            STREAMS_BLOCKED_BIDI | STREAMS_BLOCKED_UNI => Frame::StreamsBlocked {
                bidirectional: frame_type == STREAMS_BLOCKED_BIDI,
                limit: get_varint(buf)?,
            },
            CONNECTION_CLOSE_TRANSPORT => {
                let error_code = get_varint(buf)?;
                let frame_type = get_varint(buf)?;
                let reason = get_reason(buf)?;
                Frame::ConnectionClose(ConnectionClose::Transport {
                    error_code,
                    frame_type,
                    reason,
                })
            }
            CONNECTION_CLOSE_APPLICATION => {
                let error_code = get_varint(buf)?;
                let reason = get_reason(buf)?;
                Frame::ConnectionClose(ConnectionClose::Application { error_code, reason })
            }
            DATAGRAM => Frame::Datagram {
                data: buf.split_to(buf.remaining()),
            },
            DATAGRAM_WITH_LENGTH => Frame::Datagram {
                data: get_bytes(buf)?,
            },
            QX_TRANSPORT_PARAMETERS => {
                let mut encoded = get_bytes(buf)?;
                Frame::TransportParameters(TransportParameters::decode(&mut encoded)?)
            }
            QX_PING => Frame::Ping {
                sequence: get_varint(buf)?,
            },
            QX_PING_RESPONSE => Frame::PingResponse {
                sequence: get_varint(buf)?,
            },
            other if PROHIBITED.contains(&other) => {
                return Err(ProtocolError::new(
                    TransportErrorCode::FrameEncodingError,
                    format!("prohibited frame type {other:#x}"),
                ));
            }
            other => {
                return Err(ProtocolError::new(
                    TransportErrorCode::FrameEncodingError,
                    format!("unknown frame type {other:#x}"),
                ));
            }
        };
        Ok(frame)
    }

    pub(crate) fn encode(&self, out: &mut BytesMut) {
        match self {
            Frame::ResetStream {
                stream_id,
                error_code,
                final_size,
            } => {
                out.put_varint(RESET_STREAM);
                out.put_varint(*stream_id);
                out.put_varint(*error_code);
                out.put_varint(*final_size);
            }
            Frame::StopSending {
                stream_id,
                error_code,
            } => {
                out.put_varint(STOP_SENDING);
                out.put_varint(*stream_id);
                out.put_varint(*error_code);
            }
            Frame::Stream {
                stream_id,
                offset,
                data,
                fin,
            } => {
                let mut frame_type = STREAM_BASE | STREAM_OFF_BIT | STREAM_LEN_BIT;
                if *fin {
                    frame_type |= STREAM_FIN_BIT;
                }
                out.put_varint(frame_type);
                out.put_varint(*stream_id);
                out.put_varint(*offset);
                out.put_varint(data.len() as u64);
                out.put_slice(data);
            }
            Frame::MaxData { maximum } => {
                out.put_varint(MAX_DATA);
                out.put_varint(*maximum);
            }
            Frame::MaxStreamData { stream_id, maximum } => {
                out.put_varint(MAX_STREAM_DATA);
                out.put_varint(*stream_id);
                out.put_varint(*maximum);
            }
            Frame::MaxStreams {
                bidirectional,
                maximum,
            } => {
                out.put_varint(if *bidirectional {
                    MAX_STREAMS_BIDI
                } else {
                    MAX_STREAMS_UNI
                });
                out.put_varint(*maximum);
            }
            Frame::DataBlocked { limit } => {
                out.put_varint(DATA_BLOCKED);
                out.put_varint(*limit);
            }
            Frame::StreamDataBlocked { stream_id, limit } => {
                out.put_varint(STREAM_DATA_BLOCKED);
                out.put_varint(*stream_id);
                out.put_varint(*limit);
            }
            Frame::StreamsBlocked {
                bidirectional,
                limit,
            } => {
                out.put_varint(if *bidirectional {
                    STREAMS_BLOCKED_BIDI
                } else {
                    STREAMS_BLOCKED_UNI
                });
                out.put_varint(*limit);
            }
            Frame::ConnectionClose(ConnectionClose::Transport {
                error_code,
                frame_type,
                reason,
            }) => {
                out.put_varint(CONNECTION_CLOSE_TRANSPORT);
                out.put_varint(*error_code);
                out.put_varint(*frame_type);
                out.put_string(reason);
            }
            Frame::ConnectionClose(ConnectionClose::Application { error_code, reason }) => {
                out.put_varint(CONNECTION_CLOSE_APPLICATION);
                out.put_varint(*error_code);
                out.put_string(reason);
            }
            Frame::Datagram { data } => {
                out.put_varint(DATAGRAM_WITH_LENGTH);
                out.put_varint(data.len() as u64);
                out.put_slice(data);
            }
            Frame::TransportParameters(parameters) => {
                let encoded = parameters.encode();
                out.put_varint(QX_TRANSPORT_PARAMETERS);
                out.put_varint(encoded.len() as u64);
                out.put_slice(&encoded);
            }
            Frame::Ping { sequence } => {
                out.put_varint(QX_PING);
                out.put_varint(*sequence);
            }
            Frame::PingResponse { sequence } => {
                out.put_varint(QX_PING_RESPONSE);
                out.put_varint(*sequence);
            }
        }
    }

    pub(crate) fn encoded_len(&self) -> usize {
        let mut out = BytesMut::new();
        self.encode(&mut out);
        out.len()
    }
}

/// The bytes a STREAM frame adds to `data_len` bytes of payload.
pub(crate) fn stream_frame_overhead(stream_id: u64, offset: u64, data_len: usize) -> usize {
    1 + varint_len(stream_id) + varint_len(offset) + varint_len(data_len as u64)
}

/// The bytes a DATAGRAM frame adds to `data_len` bytes of payload.
pub(crate) fn datagram_frame_overhead(data_len: usize) -> usize {
    1 + varint_len(data_len as u64)
}

pub(crate) fn varint_len(value: u64) -> usize {
    match value {
        0..=0x3f => 1,
        0x40..=0x3fff => 2,
        0x4000..=0x3fff_ffff => 4,
        _ => 8,
    }
}

fn get_varint(buf: &mut Bytes) -> Result<u64, ProtocolError> {
    buf.try_get_varint()
        .map_err(|_| truncated("a variable-length integer"))
}

/// A length-prefixed byte field.
fn get_bytes(buf: &mut Bytes) -> Result<Bytes, ProtocolError> {
    let length = get_varint(buf)? as usize;
    if buf.remaining() < length {
        return Err(truncated("a length-prefixed field"));
    }
    Ok(buf.split_to(length))
}

fn get_reason(buf: &mut Bytes) -> Result<String, ProtocolError> {
    let bytes = get_bytes(buf)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn truncated(what: &str) -> ProtocolError {
    ProtocolError::new(
        TransportErrorCode::FrameEncodingError,
        format!("record ends inside {what}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(frame: Frame) -> Vec<Frame> {
        let mut out = BytesMut::new();
        frame.encode(&mut out);
        Frame::decode_record(out.freeze()).unwrap()
    }

    #[test]
    fn every_frame_survives_a_round_trip() {
        // Arrange
        let frames = vec![
            Frame::ResetStream {
                stream_id: 2,
                error_code: 7,
                final_size: 1_000_000,
            },
            Frame::StopSending {
                stream_id: 3,
                error_code: 1,
            },
            Frame::Stream {
                stream_id: 4,
                offset: 16_384,
                data: Bytes::from_static(b"payload"),
                fin: true,
            },
            Frame::MaxData { maximum: 1 << 24 },
            Frame::MaxStreamData {
                stream_id: 5,
                maximum: 1 << 20,
            },
            Frame::MaxStreams {
                bidirectional: false,
                maximum: 1000,
            },
            Frame::DataBlocked { limit: 10 },
            Frame::StreamDataBlocked {
                stream_id: 6,
                limit: 11,
            },
            Frame::StreamsBlocked {
                bidirectional: true,
                limit: 100,
            },
            Frame::ConnectionClose(ConnectionClose::Transport {
                error_code: 0x0a,
                frame_type: 0x08,
                reason: "out of order".into(),
            }),
            Frame::ConnectionClose(ConnectionClose::Application {
                error_code: 0x10,
                reason: "goodbye".into(),
            }),
            Frame::Datagram {
                data: Bytes::from_static(b"dgram"),
            },
            Frame::TransportParameters(TransportParameters::default()),
            Frame::Ping { sequence: 9 },
            Frame::PingResponse { sequence: 9 },
        ];

        // Act
        let mut record = BytesMut::new();
        for frame in &frames {
            frame.encode(&mut record);
        }
        let decoded = Frame::decode_record(record.freeze()).unwrap();

        // Assert
        assert_eq!(decoded, frames);
    }

    #[test]
    fn padding_is_skipped_between_frames() {
        // Arrange
        let mut record = BytesMut::from(&[0u8, 0, 0][..]);
        Frame::MaxData { maximum: 1 }.encode(&mut record);
        record.put_u8(0);

        // Act
        let decoded = Frame::decode_record(record.freeze()).unwrap();

        // Assert
        assert_eq!(decoded, vec![Frame::MaxData { maximum: 1 }]);
    }

    #[test]
    fn stream_frames_without_offset_and_length_run_to_the_end_of_the_record() {
        // Arrange: type 0x09 = STREAM with FIN, no OFF, no LEN
        let record = Bytes::from_static(&[0x09, 0x04, b'a', b'b']);

        // Act
        let decoded = Frame::decode_record(record).unwrap();

        // Assert
        assert_eq!(
            decoded,
            vec![Frame::Stream {
                stream_id: 4,
                offset: 0,
                data: Bytes::from_static(b"ab"),
                fin: true,
            }]
        );
    }

    #[test]
    fn a_truncated_frame_is_a_frame_encoding_error() {
        // Arrange: STREAM with LEN claiming 10 bytes, 2 present
        let record = Bytes::from_static(&[0x0a, 0x04, 0x0a, b'a', b'b']);

        // Act
        let error = Frame::decode_record(record).unwrap_err();

        // Assert
        assert_eq!(error.code, TransportErrorCode::FrameEncodingError);
    }

    #[test]
    fn prohibited_and_unknown_frame_types_are_frame_encoding_errors() {
        // Act
        let ping = Frame::decode_record(Bytes::from_static(&[0x01])).unwrap_err();
        let unknown = Frame::decode_record(Bytes::from_static(&[0x21])).unwrap_err();

        // Assert
        assert_eq!(ping.code, TransportErrorCode::FrameEncodingError);
        assert_eq!(unknown.code, TransportErrorCode::FrameEncodingError);
    }

    #[test]
    fn the_transport_parameters_frame_type_is_the_documented_wire_bytes() {
        // Act
        let mut out = BytesMut::new();
        out.put_varint(QX_TRANSPORT_PARAMETERS);

        // Assert
        assert_eq!(&out[..], b"\xffQS0\r\n\r\n");
    }

    #[test]
    fn frame_overheads_match_the_encoding() {
        // Arrange
        let data = Bytes::from(vec![0u8; 300]);

        // Act / Assert
        assert_eq!(
            stream_frame_overhead(4, 70_000, data.len()),
            Frame::Stream {
                stream_id: 4,
                offset: 70_000,
                data: data.clone(),
                fin: false,
            }
            .encoded_len()
                - data.len()
        );
        assert_eq!(
            datagram_frame_overhead(data.len()),
            Frame::Datagram { data: data.clone() }.encoded_len() - data.len()
        );
        assert_eq!(round_trip(Frame::Ping { sequence: 1 }).len(), 1);
    }
}
