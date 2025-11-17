use bytes::Bytes;
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::modules::extensions::{bytes_reader::BytesReader, bytes_writer::BytesWriter};

#[repr(u64)]
#[derive(Debug, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Copy)]
enum AliasType {
    Delete = 0x00,
    Register = 0x01,
    UseAlias = 0x02,
    UseValue = 0x03,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AuthorizationToken {
    Delete,
    Register {
        token_alias: u64,
        token_type: u64,
        token_value: Bytes,
    },
    UseAlias {
        token_alias: u64,
    },
    UseValue {
        token_type: u64,
        token_value: Bytes,
    },
}

impl AuthorizationToken {
    pub(crate) fn decode(buf: &mut bytes::BytesMut) -> Option<Self> {
        let token_alias = buf.try_get_varint().ok()?;
        if let Ok(token_alias) = AliasType::try_from(token_alias) {
            match token_alias {
                AliasType::Delete => Some(AuthorizationToken::Delete),
                AliasType::Register => {
                    let token_alias = buf.try_get_varint().ok()?;
                    let token_type = buf.try_get_varint().ok()?;
                    let token_value = Bytes::copy_from_slice(buf);
                    Some(AuthorizationToken::Register {
                        token_alias,
                        token_type,
                        token_value,
                    })
                }
                AliasType::UseAlias => {
                    let token_alias = buf.try_get_varint().ok()?;
                    Some(AuthorizationToken::UseAlias { token_alias })
                }
                AliasType::UseValue => {
                    let token_type = buf.try_get_varint().ok()?;
                    let token_value = Bytes::copy_from_slice(buf);
                    Some(AuthorizationToken::UseValue {
                        token_type,
                        token_value,
                    })
                }
            }
        } else {
            tracing::error!("Invalid alias type: {}", token_alias);
            None
        }
    }

    pub(crate) fn encode(&self) -> bytes::BytesMut {
        match self {
            AuthorizationToken::Delete => {
                let mut buf = bytes::BytesMut::new();
                buf.put_varint(AliasType::Delete as u64);
                buf
            }
            AuthorizationToken::Register {
                token_alias,
                token_type,
                token_value,
            } => {
                let mut buf = bytes::BytesMut::new();
                buf.put_varint(AliasType::Register as u64);
                buf.put_varint(*token_alias);
                buf.put_varint(*token_type);
                buf.extend_from_slice(token_value);
                buf
            }
            AuthorizationToken::UseAlias { token_alias } => {
                let mut buf = bytes::BytesMut::new();
                buf.put_varint(AliasType::UseAlias as u64);
                buf.put_varint(*token_alias);
                buf
            }
            AuthorizationToken::UseValue {
                token_type,
                token_value,
            } => {
                let mut buf = bytes::BytesMut::new();
                buf.put_varint(AliasType::UseValue as u64);
                buf.put_varint(*token_type);
                buf.extend_from_slice(token_value);
                buf
            }
        }
    }
}
