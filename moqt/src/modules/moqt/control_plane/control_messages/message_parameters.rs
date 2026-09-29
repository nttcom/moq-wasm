use std::io::Cursor;

use bytes::BytesMut;

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::control_messages::{
        key_value_pair::{KeyValuePair, VariantType},
        messages::parameters::authorization_token::AuthorizationToken,
    },
};

const DELIVERY_TIMEOUT: u64 = 0x02;
const AUTHORIZATION_TOKEN: u64 = 0x03;
const MAX_CACHE_DURATION: u64 = 0x04;

/// Message Parameters, draft-ietf-moq-transport-14 §9.2.1.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MessageParameters {
    pub(crate) authorization_tokens: Vec<AuthorizationToken>,
    pub(crate) delivery_timeout: Option<u64>,
    pub(crate) max_cache_duration: Option<u64>,
}

impl MessageParameters {
    pub(crate) fn decode(buf: &mut Cursor<&[u8]>) -> Option<Self> {
        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;
        let mut parameters = Self::default();
        for _ in 0..number_of_parameters {
            match KeyValuePair::decode(buf)? {
                KeyValuePair {
                    key: AUTHORIZATION_TOKEN,
                    value: VariantType::Odd(value),
                } => {
                    if let Some(token) = AuthorizationToken::decode(&mut Cursor::new(&value[..])) {
                        parameters.authorization_tokens.push(token);
                    }
                }
                KeyValuePair {
                    key: DELIVERY_TIMEOUT,
                    value: VariantType::Even(value),
                } => {
                    parameters.delivery_timeout.get_or_insert(value);
                }
                KeyValuePair {
                    key: MAX_CACHE_DURATION,
                    value: VariantType::Even(value),
                } => {
                    parameters.max_cache_duration.get_or_insert(value);
                }
                _ => {}
            }
        }
        Some(parameters)
    }

    pub(crate) fn encode(&self) -> BytesMut {
        let mut pairs: Vec<KeyValuePair> = self
            .authorization_tokens
            .iter()
            .map(|token| KeyValuePair {
                key: AUTHORIZATION_TOKEN,
                value: VariantType::Odd(token.encode().freeze()),
            })
            .collect();
        if let Some(delivery_timeout) = self.delivery_timeout {
            pairs.push(KeyValuePair {
                key: DELIVERY_TIMEOUT,
                value: VariantType::Even(delivery_timeout),
            });
        }
        if let Some(max_cache_duration) = self.max_cache_duration {
            pairs.push(KeyValuePair {
                key: MAX_CACHE_DURATION,
                value: VariantType::Even(max_cache_duration),
            });
        }
        let mut payload = BytesMut::new();
        payload.put_varint(pairs.len() as u64);
        for pair in pairs {
            payload.unsplit(pair.encode());
        }
        payload
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use bytes::BytesMut;

    use super::MessageParameters;
    use crate::modules::{
        extensions::buf_put_ext::BufPutExt,
        moqt::control_plane::control_messages::{
            key_value_pair::{KeyValuePair, VariantType},
            messages::parameters::authorization_token::AuthorizationToken,
        },
    };

    fn decode(bytes: &[u8]) -> MessageParameters {
        MessageParameters::decode(&mut Cursor::new(bytes)).unwrap()
    }

    #[test]
    fn every_parameter_round_trips() {
        // Arrange
        let parameters = MessageParameters {
            authorization_tokens: vec![
                AuthorizationToken::use_value_utf8("a"),
                AuthorizationToken::use_value_utf8("b"),
            ],
            delivery_timeout: Some(500),
            max_cache_duration: Some(60_000),
        };

        // Act
        let decoded = decode(&parameters.encode());

        // Assert
        assert_eq!(decoded, parameters);
    }

    #[test]
    fn no_parameters_encode_as_a_zero_count() {
        // Act / Assert
        assert_eq!(MessageParameters::default().encode().as_ref(), &[0]);
    }

    #[test]
    fn unknown_parameter_is_skipped() {
        // Arrange
        let mut bytes = BytesMut::new();
        bytes.put_varint(2);
        bytes.unsplit(
            KeyValuePair {
                key: 0x3e,
                value: VariantType::Even(7),
            }
            .encode(),
        );
        bytes.unsplit(
            KeyValuePair {
                key: 0x02,
                value: VariantType::Even(9),
            }
            .encode(),
        );

        // Act
        let decoded = decode(&bytes);

        // Assert
        assert_eq!(decoded.delivery_timeout, Some(9));
    }
}
