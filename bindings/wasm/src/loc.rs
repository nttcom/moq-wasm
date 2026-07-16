use anyhow::Result;
use moqt::wire::ExtensionHeaders;
use packages::loc::LocHeader;

pub fn loc_header_to_extension_headers(header: &LocHeader) -> ExtensionHeaders {
    header.to_extension_headers()
}

pub fn extension_headers_to_loc_header(headers: &ExtensionHeaders) -> LocHeader {
    LocHeader::from_extension_headers(headers)
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
