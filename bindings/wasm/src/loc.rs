use anyhow::{Result, anyhow};
use mediapack::loc::{LocExtension, to_extension_headers};
use moqt::wire::ExtensionHeaders;
use wasm_bindgen::JsValue;

use crate::js_error;

pub fn parse_loc_header(value: JsValue) -> Result<ExtensionHeaders, JsValue> {
    if value.is_undefined() || value.is_null() {
        return Ok(ExtensionHeaders::default());
    }
    let extensions: Vec<LocExtension> = serde_wasm_bindgen::from_value(value)
        .map_err(|err| js_error(format!("invalid loc header: {err}")))?;
    Ok(to_extension_headers(&extensions))
}

pub fn encode_loc_header(extensions: &[LocExtension]) -> Result<JsValue> {
    serde_wasm_bindgen::to_value(extensions).map_err(|err| anyhow!("loc header: {err}"))
}
