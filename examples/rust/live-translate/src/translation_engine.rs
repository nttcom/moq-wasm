use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    echo_translation::EchoTranslation,
    gemini::{AgentPlatform, GeminiTranslation},
    target_language::TargetLanguage,
};

pub(crate) enum TranslationOutput {
    ConnectionStarted,
    Audio(Vec<i16>),
    SourceTranscript(Transcript),
    TranslatedTranscript(Transcript),
}

/// A piece of an utterance's text; `finished` marks the last piece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transcript {
    pub(crate) text: String,
    pub(crate) finished: bool,
}

#[derive(Clone)]
pub(crate) enum TranslationEngine {
    Gemini { platform: Arc<AgentPlatform> },
    Echo,
}

pub(crate) enum RunningTranslation {
    Gemini { _translation: GeminiTranslation },
    Echo { _translation: EchoTranslation },
}

impl TranslationEngine {
    pub(crate) fn start(
        &self,
        language: &'static TargetLanguage,
        pcm_receiver: mpsc::Receiver<Vec<i16>>,
        translated_sender: mpsc::Sender<TranslationOutput>,
    ) -> RunningTranslation {
        match self {
            Self::Gemini { platform } => RunningTranslation::Gemini {
                _translation: GeminiTranslation::run(
                    platform.clone(),
                    language.code,
                    pcm_receiver,
                    translated_sender,
                ),
            },
            Self::Echo => RunningTranslation::Echo {
                _translation: EchoTranslation::run(
                    language.gemini_delay,
                    pcm_receiver,
                    translated_sender,
                ),
            },
        }
    }
}
