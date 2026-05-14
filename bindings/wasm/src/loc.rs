use anyhow::Result;
use bytes::Bytes;
use moqt::wire::ExtensionHeaders;
use packages::loc::LocHeader;

const LOC_HEADER_SENTINEL: &[u8] = b"loc:";

pub fn loc_header_to_extension_headers(header: &LocHeader) -> Result<ExtensionHeaders> {
    let mut immutable_extensions = Vec::with_capacity(1);
    let mut encoded = Vec::from(LOC_HEADER_SENTINEL);
    encoded.extend_from_slice(&serde_json::to_vec(header)?);
    immutable_extensions.push(Bytes::from(encoded));

    Ok(ExtensionHeaders {
        prior_group_id_gap: vec![],
        prior_object_id_gap: vec![],
        immutable_extensions,
    })
}

pub fn extension_headers_to_loc_header(headers: &ExtensionHeaders) -> LocHeader {
    for extension in &headers.immutable_extensions {
        if let Some(encoded) = extension.as_ref().strip_prefix(LOC_HEADER_SENTINEL)
            && let Ok(header) = serde_json::from_slice::<LocHeader>(encoded)
        {
            return header;
        }
    }

    LocHeader::default()
}

pub fn parse_loc_header(value: wasm_bindgen::JsValue) -> Result<Option<LocHeader>> {
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }

    let header: LocHeader = serde_wasm_bindgen::from_value(value)
        .map_err(|err| anyhow::anyhow!("invalid loc header: {err}"))?;
    Ok(Some(header))
}

pub fn encode_loc_header(header: &LocHeader) -> Result<wasm_bindgen::JsValue> {
    serde_wasm_bindgen::to_value(header).map_err(|err| anyhow::anyhow!("loc header: {err}"))
}
