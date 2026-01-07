use bytes::Bytes;

use crate::modules::{
    extensions::{buf_get_ext::BufGetExt, buf_put_ext::BufPutExt, result_ext::ResultExt},
    moqt::control_plane::messages::control_messages::key_value_pair::{KeyValuePair, VariantType},
};

pub enum ExtensionHeaderType {
    PriorGroupIdGap = 0x3c,
    PriorObjectIdGap = 0x3e,
    ImmutableExtensions = 0xb,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionHeaders {
    pub prior_group_id_gap: Vec<u64>,
    pub prior_object_id_gap: Vec<u64>,
    pub immutable_extensions: Vec<Bytes>,
}

impl ExtensionHeaders {
    pub fn decode(buf: &mut bytes::BytesMut) -> Option<Self> {
        let mut kv_pairs = Vec::new();
        let number_of_parameters = buf
            .try_get_varint()
            .log_context("number of parameters")
            .ok()?;
        for _ in 0..number_of_parameters {
            let key_value_pair = KeyValuePair::decode(buf)?;
            kv_pairs.push(key_value_pair);
        }
        let prior_group_id_gap = kv_pairs
            .iter()
            .filter(|item| item.key == ExtensionHeaderType::PriorGroupIdGap as u64)
            .map(|kv_pair| match &kv_pair.value {
                VariantType::Odd(_) => unreachable!(),
                VariantType::Even(v) => *v,
            })
            .collect();
        let prior_object_id_gap = kv_pairs
            .iter()
            .filter(|item| item.key == ExtensionHeaderType::PriorObjectIdGap as u64)
            .map(|kv_pair| match &kv_pair.value {
                VariantType::Odd(_) => unreachable!(),
                VariantType::Even(v) => *v,
            })
            .collect();
        let immutable_extensions = kv_pairs
            .iter()
            .filter(|kv_pair| kv_pair.key == ExtensionHeaderType::ImmutableExtensions as u64)
            .map(|kv_pair| match &kv_pair.value {
                VariantType::Odd(value) => value.clone(),
                VariantType::Even(_) => unreachable!(),
            })
            .collect();

        Some(Self {
            prior_group_id_gap,
            prior_object_id_gap,
            immutable_extensions,
        })
    }

    pub fn encode(&self) -> bytes::BytesMut {
        let mut buf = bytes::BytesMut::new();
        let total_parameters = self.prior_group_id_gap.len()
            + self.prior_object_id_gap.len()
            + self.immutable_extensions.len();
        buf.put_varint(total_parameters as u64);
        for gap in &self.prior_group_id_gap {
            buf.put_varint(ExtensionHeaderType::PriorGroupIdGap as u64);
            buf.put_varint(*gap);
        }
        for gap in &self.prior_object_id_gap {
            buf.put_varint(ExtensionHeaderType::PriorObjectIdGap as u64);
            buf.put_varint(*gap);
        }
        for ext in &self.immutable_extensions {
            buf.put_varint(ExtensionHeaderType::ImmutableExtensions as u64);
            buf.put_varint(ext.len() as u64);
            buf.extend_from_slice(ext);
        }
        buf
    }
}
