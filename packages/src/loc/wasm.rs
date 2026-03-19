use serde_wasm_bindgen as swb;
use wasm_bindgen::prelude::*;

use super::{LocHeader, LocObject};

#[wasm_bindgen]
pub struct LocHeaderWasm {
    inner: LocHeader,
}

#[wasm_bindgen]
impl LocHeaderWasm {
    #[wasm_bindgen(constructor)]
    pub fn new(value: JsValue) -> Result<LocHeaderWasm, JsValue> {
        let header: LocHeader = swb::from_value(value).map_err(to_js_error)?;
        Ok(LocHeaderWasm { inner: header })
    }

    pub fn to_js(&self) -> Result<JsValue, JsValue> {
        swb::to_value(&self.inner).map_err(to_js_error)
    }
}

#[wasm_bindgen]
pub struct LocObjectWasm {
    inner: LocObject,
}

#[wasm_bindgen]
impl LocObjectWasm {
    #[wasm_bindgen(constructor)]
    pub fn new(value: JsValue) -> Result<LocObjectWasm, JsValue> {
        let object: LocObject = swb::from_value(value).map_err(to_js_error)?;
        Ok(LocObjectWasm { inner: object })
    }

    #[wasm_bindgen(getter)]
    pub fn header(&self) -> Result<JsValue, JsValue> {
        swb::to_value(&self.inner.header).map_err(to_js_error)
    }

    #[wasm_bindgen(getter)]
    pub fn payload(&self) -> Vec<u8> {
        self.inner.payload.clone()
    }

    pub fn to_js(&self) -> Result<JsValue, JsValue> {
        swb::to_value(&self.inner).map_err(to_js_error)
    }
}

fn to_js_error<E: std::fmt::Display>(err: E) -> JsValue {
    JsValue::from_str(&err.to_string())
}
