use anyhow::Result;
use moqt_core::messages::data_streams::extension_header::{
    ExtensionHeader, ExtensionHeaderValue, Value, ValueWithLength,
};
use packages::loc::{
    AudioLevel, CaptureTimestamp, LocHeader, LocHeaderExtension, LocHeaderValue,
    UnknownHeaderExtension, VideoConfig, VideoFrameMarking, LOC_AUDIO_LEVEL_ID,
    LOC_CAPTURE_TIMESTAMP_ID, LOC_VIDEO_CONFIG_ID, LOC_VIDEO_FRAME_MARKING_ID,
};

pub fn loc_header_to_extension_headers(header: &LocHeader) -> Result<Vec<ExtensionHeader>> {
    let mut headers = Vec::with_capacity(header.extensions.len());
    for ext in &header.extensions {
        match ext {
            LocHeaderExtension::CaptureTimestamp(value) => {
                let bytes = value.micros_since_unix_epoch.to_be_bytes().to_vec();
                headers.push(ExtensionHeader::new(
                    LOC_CAPTURE_TIMESTAMP_ID,
                    ExtensionHeaderValue::EvenTypeValue(ValueWithLength::new(bytes)),
                )?);
            }
            LocHeaderExtension::VideoConfig(value) => {
                headers.push(ExtensionHeader::new(
                    LOC_VIDEO_CONFIG_ID,
                    ExtensionHeaderValue::EvenTypeValue(ValueWithLength::new(value.data.clone())),
                )?);
            }
            LocHeaderExtension::VideoFrameMarking(value) => {
                headers.push(ExtensionHeader::new(
                    LOC_VIDEO_FRAME_MARKING_ID,
                    ExtensionHeaderValue::EvenTypeValue(ValueWithLength::new(value.data.clone())),
                )?);
            }
            LocHeaderExtension::AudioLevel(value) => {
                headers.push(ExtensionHeader::new(
                    LOC_AUDIO_LEVEL_ID,
                    ExtensionHeaderValue::EvenTypeValue(ValueWithLength::new(vec![value.level])),
                )?);
            }
            LocHeaderExtension::Unknown(value) => {
                let header = match &value.value {
                    LocHeaderValue::EvenBytes(bytes) => ExtensionHeader::new(
                        value.id,
                        ExtensionHeaderValue::EvenTypeValue(ValueWithLength::new(bytes.clone())),
                    )?,
                    LocHeaderValue::OddVarint(varint) => ExtensionHeader::new(
                        value.id,
                        ExtensionHeaderValue::OddTypeValue(Value::new(*varint)),
                    )?,
                };
                headers.push(header);
            }
        }
    }
    Ok(headers)
}

pub fn extension_headers_to_loc_header(headers: &[ExtensionHeader]) -> LocHeader {
    let mut extensions = Vec::with_capacity(headers.len());
    for header in headers {
        let id = header.header_type();
        let ext = match id {
            LOC_CAPTURE_TIMESTAMP_ID => match header.value() {
                ExtensionHeaderValue::EvenTypeValue(value) => {
                    let bytes = value.header_value();
                    if bytes.len() == 8 {
                        let mut buf = [0u8; 8];
                        buf.copy_from_slice(bytes);
                        LocHeaderExtension::CaptureTimestamp(CaptureTimestamp {
                            micros_since_unix_epoch: u64::from_be_bytes(buf),
                        })
                    } else {
                        LocHeaderExtension::Unknown(to_unknown(id, header.value()))
                    }
                }
                _ => LocHeaderExtension::Unknown(to_unknown(id, header.value())),
            },
            LOC_VIDEO_CONFIG_ID => match header.value() {
                ExtensionHeaderValue::EvenTypeValue(value) => {
                    LocHeaderExtension::VideoConfig(VideoConfig {
                        data: value.header_value().to_vec(),
                    })
                }
                _ => LocHeaderExtension::Unknown(to_unknown(id, header.value())),
            },
            LOC_VIDEO_FRAME_MARKING_ID => match header.value() {
                ExtensionHeaderValue::EvenTypeValue(value) => {
                    LocHeaderExtension::VideoFrameMarking(VideoFrameMarking {
                        data: value.header_value().to_vec(),
                    })
                }
                _ => LocHeaderExtension::Unknown(to_unknown(id, header.value())),
            },
            LOC_AUDIO_LEVEL_ID => match header.value() {
                ExtensionHeaderValue::EvenTypeValue(value) => {
                    let bytes = value.header_value();
                    if bytes.len() == 1 {
                        LocHeaderExtension::AudioLevel(AudioLevel { level: bytes[0] })
                    } else {
                        LocHeaderExtension::Unknown(to_unknown(id, header.value()))
                    }
                }
                _ => LocHeaderExtension::Unknown(to_unknown(id, header.value())),
            },
            _ => LocHeaderExtension::Unknown(to_unknown(id, header.value())),
        };
        extensions.push(ext);
    }
    LocHeader { extensions }
}

fn to_unknown(id: u64, value: &ExtensionHeaderValue) -> UnknownHeaderExtension {
    let loc_value = match value {
        ExtensionHeaderValue::EvenTypeValue(value) => {
            LocHeaderValue::EvenBytes(value.header_value().to_vec())
        }
        ExtensionHeaderValue::OddTypeValue(value) => LocHeaderValue::OddVarint(value.header_value()),
    };
    UnknownHeaderExtension { id, value: loc_value }
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
