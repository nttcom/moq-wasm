use wasm_bindgen::prelude::*;

use msf::Catalog;

#[wasm_bindgen]
pub fn parse_msf_catalog_json(json: &str) -> Result<JsValue, JsValue> {
    let catalog: Catalog =
        serde_json::from_str(json).map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_wasm_bindgen::to_value(&catalog).map_err(|err| JsValue::from_str(&err.to_string()))
}
