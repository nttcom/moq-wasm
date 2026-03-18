use wasm_bindgen::prelude::*;

use media_streaming_format::Catalog;

#[wasm_bindgen]
pub fn parse_msf_catalog_json(json: &str) -> Result<JsValue, JsValue> {
    let catalog: Catalog =
        serde_json::from_str(json).map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_wasm_bindgen::to_value(&catalog).map_err(|err| JsValue::from_str(&err.to_string()))
}

#[wasm_bindgen]
pub fn msf_catalog_to_json(value: JsValue) -> Result<String, JsValue> {
    let catalog: Catalog =
        serde_wasm_bindgen::from_value(value).map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_json::to_string(&catalog).map_err(|err| JsValue::from_str(&err.to_string()))
}
