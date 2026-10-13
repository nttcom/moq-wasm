use anyhow::Context;
use std::fmt;

use bytes::{Buf, Bytes};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::modules::extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt};

#[repr(u64)]
#[derive(Debug, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, Copy)]
enum AliasType {
    Delete = 0x00,
    Register = 0x01,
    UseAlias = 0x02,
    UseValue = 0x03,
}

#[derive(Clone, PartialEq)]
pub enum AuthorizationToken {
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

impl fmt::Debug for AuthorizationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Delete => f.write_str("Delete"),
            Self::Register {
                token_alias,
                token_type,
                token_value,
            } => f
                .debug_struct("Register")
                .field("token_alias", token_alias)
                .field("token_type", token_type)
                .field("token_value", &RedactedTokenValue(token_value.len()))
                .finish(),
            Self::UseAlias { token_alias } => f
                .debug_struct("UseAlias")
                .field("token_alias", token_alias)
                .finish(),
            Self::UseValue {
                token_type,
                token_value,
            } => f
                .debug_struct("UseValue")
                .field("token_type", token_type)
                .field("token_value", &RedactedTokenValue(token_value.len()))
                .finish(),
        }
    }
}

struct RedactedTokenValue(usize);

impl fmt::Debug for RedactedTokenValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes redacted>", self.0)
    }
}

impl AuthorizationToken {
    pub fn use_value_utf8(value: &str) -> Self {
        Self::UseValue {
            token_type: 0,
            token_value: Bytes::copy_from_slice(value.as_bytes()),
        }
    }

    pub fn decode(buf: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<Self> {
        let token_alias = buf.try_get_varint().context("alias type")?;
        if let Ok(token_alias) = AliasType::try_from(token_alias) {
            match token_alias {
                AliasType::Delete => Ok(AuthorizationToken::Delete),
                AliasType::Register => {
                    let token_alias = buf.try_get_varint().context("token alias")?;
                    let token_type = buf.try_get_varint().context("token type")?;
                    let token_value = buf.copy_to_bytes(buf.remaining());
                    Ok(AuthorizationToken::Register {
                        token_alias,
                        token_type,
                        token_value,
                    })
                }
                AliasType::UseAlias => {
                    let token_alias = buf.try_get_varint().context("token alias")?;
                    Ok(AuthorizationToken::UseAlias { token_alias })
                }
                AliasType::UseValue => {
                    let token_type = buf.try_get_varint().context("token type")?;
                    let token_value = buf.copy_to_bytes(buf.remaining());
                    Ok(AuthorizationToken::UseValue {
                        token_type,
                        token_value,
                    })
                }
            }
        } else {
            anyhow::bail!("Invalid alias type: {}", token_alias)
        }
    }

    pub(crate) fn decode_parameter(value: &[u8]) -> Option<Self> {
        Self::decode(&mut std::io::Cursor::new(value))
            .inspect_err(|error| {
                tracing::warn!(error = %format!("{error:#}"), "dropping a malformed authorization token")
            })
            .ok()
    }

    pub fn encode(&self) -> bytes::BytesMut {
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

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "eyJhbGciOiJFUzI1NiJ9.payload.signature";

    #[test]
    fn debug_output_of_a_use_value_token_hides_the_value() {
        // Arrange
        let token = AuthorizationToken::use_value_utf8(TOKEN);

        // Act
        let debug = format!("{token:?}");

        // Assert
        assert_eq!(
            debug,
            format!(
                "UseValue {{ token_type: 0, token_value: <{} bytes redacted> }}",
                TOKEN.len()
            )
        );
    }

    #[test]
    fn debug_output_of_a_register_token_hides_the_value() {
        // Arrange
        let token = AuthorizationToken::Register {
            token_alias: 1,
            token_type: 0,
            token_value: Bytes::from_static(TOKEN.as_bytes()),
        };

        // Act
        let debug = format!("{token:?}");

        // Assert
        assert!(!debug.contains(TOKEN), "{debug}");
    }
}
