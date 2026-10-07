use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

const MODEL: &str = "models/gemini-3.5-live-translate-preview";
const INPUT_MIME_TYPE: &str = "audio/pcm;rate=16000";

#[derive(Serialize)]
struct SetupMessage<'a> {
    setup: Setup<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Setup<'a> {
    model: &'a str,
    generation_config: GenerationConfig<'a>,
    input_audio_transcription: AudioTranscription,
    output_audio_transcription: AudioTranscription,
    session_resumption: SessionResumption<'a>,
    context_window_compression: ContextWindowCompression,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig<'a> {
    response_modalities: [&'a str; 1],
    translation_config: TranslationConfig<'a>,
}

#[derive(Serialize)]
struct AudioTranscription {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslationConfig<'a> {
    target_language_code: &'a str,
    echo_target_language: bool,
}

#[derive(Serialize)]
struct SessionResumption<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    handle: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextWindowCompression {
    sliding_window: SlidingWindow,
}

#[derive(Serialize)]
struct SlidingWindow {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RealtimeInputMessage<'a> {
    realtime_input: RealtimeInput<'a>,
}

#[derive(Serialize)]
struct RealtimeInput<'a> {
    audio: Blob<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Blob<'a> {
    data: String,
    mime_type: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerMessage {
    setup_complete: Option<serde_json::Value>,
    server_content: Option<ServerContent>,
    go_away: Option<serde_json::Value>,
    session_resumption_update: Option<SessionResumptionUpdate>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerContent {
    model_turn: Option<ModelTurn>,
    input_transcription: Option<Transcription>,
    output_transcription: Option<Transcription>,
}

#[derive(Deserialize)]
struct Transcription {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct ModelTurn {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    inline_data: Option<InlineData>,
}

#[derive(Deserialize)]
struct InlineData {
    data: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionResumptionUpdate {
    new_handle: Option<String>,
    #[serde(default)]
    resumable: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ServerEvent {
    SetupComplete,
    TranslatedAudio(Vec<i16>),
    InputTranscript(String),
    OutputTranscript(String),
    GoAway,
    ResumptionHandle(String),
}

pub(crate) fn setup_message(target_language_code: &str, resumption_handle: Option<&str>) -> String {
    let message = SetupMessage {
        setup: Setup {
            model: MODEL,
            generation_config: GenerationConfig {
                response_modalities: ["AUDIO"],
                translation_config: TranslationConfig {
                    target_language_code,
                    echo_target_language: true,
                },
            },
            input_audio_transcription: AudioTranscription {},
            output_audio_transcription: AudioTranscription {},
            session_resumption: SessionResumption {
                handle: resumption_handle,
            },
            context_window_compression: ContextWindowCompression {
                sliding_window: SlidingWindow {},
            },
        },
    };
    serde_json::to_string(&message).expect("setup message is always serializable")
}

pub(crate) fn audio_message(samples: &[i16]) -> String {
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    let message = RealtimeInputMessage {
        realtime_input: RealtimeInput {
            audio: Blob {
                data: STANDARD.encode(bytes),
                mime_type: INPUT_MIME_TYPE,
            },
        },
    };
    serde_json::to_string(&message).expect("audio message is always serializable")
}

pub(crate) fn parse_server_message(text: &str) -> Result<Vec<ServerEvent>> {
    let message: ServerMessage =
        serde_json::from_str(text).context("parse Gemini server message")?;
    let mut events = Vec::new();
    if message.setup_complete.is_some() {
        events.push(ServerEvent::SetupComplete);
    }
    let (parts, input_transcription, output_transcription) = match message.server_content {
        Some(content) => (
            content
                .model_turn
                .map(|turn| turn.parts)
                .unwrap_or_default(),
            content.input_transcription,
            content.output_transcription,
        ),
        None => (Vec::new(), None, None),
    };
    if let Some(transcription) = input_transcription {
        events.push(ServerEvent::InputTranscript(transcription.text));
    }
    if let Some(transcription) = output_transcription {
        events.push(ServerEvent::OutputTranscript(transcription.text));
    }
    for inline_data in parts.into_iter().filter_map(|part| part.inline_data) {
        let bytes = STANDARD
            .decode(inline_data.data)
            .context("decode Gemini audio")?;
        let samples = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect();
        events.push(ServerEvent::TranslatedAudio(samples));
    }
    if let Some(update) = message.session_resumption_update
        && update.resumable
        && let Some(handle) = update.new_handle
    {
        events.push(ServerEvent::ResumptionHandle(handle));
    }
    if message.go_away.is_some() {
        events.push(ServerEvent::GoAway);
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_selects_the_translate_model_and_target_language() {
        // Act
        let json: serde_json::Value =
            serde_json::from_str(&setup_message("en", None)).expect("valid json");

        // Assert
        assert_eq!(json["setup"]["model"], MODEL);
        let translation = &json["setup"]["generationConfig"]["translationConfig"];
        assert_eq!(translation["targetLanguageCode"], "en");
        assert_eq!(translation["echoTargetLanguage"], true);
        assert_eq!(
            json["setup"]["outputAudioTranscription"],
            serde_json::json!({})
        );
        assert!(
            json["setup"]["generationConfig"]
                .get("outputAudioTranscription")
                .is_none()
        );
        assert_eq!(json["setup"]["sessionResumption"], serde_json::json!({}));
    }

    #[test]
    fn setup_carries_the_resumption_handle_when_reconnecting() {
        // Act
        let json: serde_json::Value =
            serde_json::from_str(&setup_message("ko", Some("handle-1"))).expect("valid json");

        // Assert
        assert_eq!(json["setup"]["sessionResumption"]["handle"], "handle-1");
    }

    #[test]
    fn audio_is_sent_as_little_endian_pcm_in_base64() {
        // Act
        let json: serde_json::Value =
            serde_json::from_str(&audio_message(&[1, -2])).expect("valid json");

        // Assert
        let audio = &json["realtimeInput"]["audio"];
        assert_eq!(audio["mimeType"], INPUT_MIME_TYPE);
        assert_eq!(audio["data"], STANDARD.encode([0x01, 0x00, 0xfe, 0xff]));
    }

    #[test]
    fn translated_audio_is_decoded_from_the_model_turn() {
        // Arrange
        let data = STANDARD.encode([0x01, 0x00, 0xfe, 0xff]);
        let text = format!(
            r#"{{"serverContent":{{"modelTurn":{{"parts":[{{"inlineData":{{"mimeType":"audio/pcm;rate=24000","data":"{data}"}}}}]}}}}}}"#
        );

        // Act
        let events = parse_server_message(&text).expect("parsed");

        // Assert
        assert_eq!(events, vec![ServerEvent::TranslatedAudio(vec![1, -2])]);
    }

    #[test]
    fn transcripts_of_both_sides_become_events() {
        // Arrange
        let text = r#"{"serverContent":{"inputTranscription":{"text":"こんにちは"},"outputTranscription":{"text":"Hello"}}}"#;

        // Act
        let events = parse_server_message(text).expect("parsed");

        // Assert
        assert_eq!(
            events,
            vec![
                ServerEvent::InputTranscript("こんにちは".to_string()),
                ServerEvent::OutputTranscript("Hello".to_string()),
            ]
        );
    }

    #[test]
    fn control_messages_become_events() {
        // Act
        let setup = parse_server_message(r#"{"setupComplete":{}}"#).expect("parsed");
        let handle = parse_server_message(
            r#"{"sessionResumptionUpdate":{"newHandle":"h2","resumable":true}}"#,
        )
        .expect("parsed");
        let not_resumable = parse_server_message(
            r#"{"sessionResumptionUpdate":{"newHandle":"h3","resumable":false}}"#,
        )
        .expect("parsed");
        let go_away = parse_server_message(r#"{"goAway":{"timeLeft":"5s"}}"#).expect("parsed");

        // Assert
        assert_eq!(setup, vec![ServerEvent::SetupComplete]);
        assert_eq!(
            handle,
            vec![ServerEvent::ResumptionHandle("h2".to_string())]
        );
        assert!(not_resumable.is_empty());
        assert_eq!(go_away, vec![ServerEvent::GoAway]);
    }
}
