use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{ReadableStreamDefaultReader, ReadableStreamReadResult};

pub(super) fn js_error(value: JsValue) -> anyhow::Error {
    anyhow::anyhow!("{value:?}")
}

/// `WebTransportError.streamErrorCode` carries the peer's RESET_STREAM or
/// STOP_SENDING code; it is absent on session-level and non-transport errors.
pub(super) fn stream_error_code(error: &JsValue) -> Option<u64> {
    js_sys::Reflect::get(error, &JsValue::from_str("streamErrorCode"))
        .ok()?
        .as_f64()
        .map(|code| code as u64)
}

pub(super) fn is_session_error(error: &JsValue) -> bool {
    js_sys::Reflect::get(error, &JsValue::from_str("source"))
        .ok()
        .and_then(|source| source.as_string())
        .is_some_and(|source| source == "session")
}

pub(super) async fn read_next(
    reader: &ReadableStreamDefaultReader,
) -> Result<Option<JsValue>, JsValue> {
    let result: ReadableStreamReadResult = JsFuture::from(reader.read()).await?.unchecked_into();
    if result.get_done() == Some(true) {
        return Ok(None);
    }
    Ok(Some(result.get_value()))
}
