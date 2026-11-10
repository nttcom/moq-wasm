use wasm_bindgen::prelude::*;

use msf::Catalog;

/// Parses an MSF catalog (draft-ietf-moq-msf-01 §5) and throws when it is not
/// one. The wasm module must be loaded: `loadMoqt`, or any `MoqSession.connect`.
#[wasm_bindgen(js_name = parseCatalog, unchecked_return_type = "unknown")]
pub fn parse_msf_catalog_json(json: &str) -> Result<JsValue, JsValue> {
    let catalog: Catalog =
        serde_json::from_str(json).map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_wasm_bindgen::to_value(&catalog).map_err(|err| JsValue::from_str(&err.to_string()))
}
