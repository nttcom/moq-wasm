use anyhow::Result;
use moqt_core::messages::data_streams::extension_header::{
    ExtensionHeader, ExtensionHeaderValue, Value, ValueWithLength,
};
use packages::loc::{
    LocHeader, LocHeaderExtension, LocHeaderValue, LOC_AUDIO_LEVEL_ID, LOC_CAPTURE_TIMESTAMP_ID,
    LOC_VIDEO_CONFIG_ID, LOC_VIDEO_FRAME_MARKING_ID,
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
