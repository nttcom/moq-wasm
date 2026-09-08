# MoQT voice pipeline server (FastAPI)

A FastAPI process that listens for MoQT sessions, decodes the audio tracks it
receives and runs each one through a `VAD → STT → LLM → TTS` pipeline. Every
stage is chosen by an environment variable, and results go back over MoQT:
transcripts and replies as JSON on `<namespace>/transcript`, synthesized
speech as Opus on `<namespace>/reply`. Transcripts are also logged; `GET /`
reports counters and the active pipeline.

## Pipeline stages

| Stage | Variable | Choices (default first) | Credentials |
| --- | --- | --- | --- |
| VAD | `PIPELINE_VAD` | `silero` (ONNX model bundled with faster-whisper), `energy` (RMS threshold) | – |
| STT | `PIPELINE_STT` | `whisper` (faster-whisper, local), `deepgram` (pre-recorded API), `openai` (audio transcriptions API), `wav` (debug: writes utterances) | `DEEPGRAM_API_KEY` / `OPENAI_API_KEY` |
| LLM | `PIPELINE_LLM` | `gemini`, `echo` (repeats the transcript, for local testing), `none` | `GEMINI_API_KEY` |
| TTS | `PIPELINE_TTS` | `gemini` (mono PCM, re-encoded to Opus), `none` | `GEMINI_API_KEY` |

The VAD cuts the 16 kHz mono stream into utterances; each utterance is
transcribed, the transcript is sent to the LLM, and the reply is synthesized.
`none` stops the pipeline after the previous stage, so `PIPELINE_LLM=none`
gives a transcription-only server. Stage failures are logged and the next
utterance is processed.

A new implementation is a class satisfying the protocol in
`stt_server/pipeline/base.py` (`VoiceActivityDetector`, `SpeechToText`,
`LanguageModel` or `TextToSpeech`) plus an entry in the matching factory table
in `stt_server/pipeline/__init__.py`.

Tuning: `STT_LANGUAGE` (default `ja`), `SILERO_THRESHOLD` (0.5; raise it for a
noisy microphone), `SILERO_MIN_SILENCE_MS` (500), `WHISPER_MODEL`
(`small`), `WHISPER_DEVICE` (`cpu`), `WHISPER_COMPUTE_TYPE` (`int8`),
`WHISPER_NO_SPEECH_THRESHOLD` (0.6, segments Whisper rates as non-speech are
dropped), `GEMINI_MODEL` (`gemini-3.6-flash`), `LLM_SYSTEM_PROMPT`,
`GEMINI_TTS_MODEL` (`gemini-3.1-flash-tts-preview`), `GEMINI_TTS_VOICE` (`Kore`).

## How audio reaches the server

- **PUBLISH** (moq-cli, bridges/live-ingest, the Python bindings): every
  track is accepted. `catalog` objects are parsed for audio tracks; a track
  named in the catalog with `role: audio` / an audio codec is processed, as
  is a track named `AUDIO_TRACK_NAME` (default `audio`) when no catalog
  arrived.
- **PUBLISH_NAMESPACE** (the browser example): the server subscribes to
  `<namespace>/catalog`, then to every audio track the catalog lists.

Object payloads may be bare `EncodedAudioChunk` bytes (browser, LOC) or the
`bridges/live-ingest` framing (4-byte length + JSON metadata + frame); the
metadata's codec wins over the catalog. Opus and AAC (`mp4a.40.x`) are
decoded with PyAV.

## Result tracks

Subscribe to `<namespace>/transcript` for JSON objects, one group each:

```json
{"type": "transcript", "track": "audio", "text": "こんにちは", "at": 1725700000.1}
{"type": "reply", "track": "audio", "text": "こんにちは。ご用件は何でしょう?", "at": 1725700001.4}
```

Subscribe to `<namespace>/reply` for the spoken reply:
one group per reply, one 20 ms Opus packet (48 kHz mono) per object. Any
other SUBSCRIBE is rejected.

## Run

```bash
cd examples/python/stt-server
uv sync
openssl req -x509 -nodes -days 1 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
  -keyout key.pem -out cert.pem -subj /CN=localhost \
  -addext "subjectAltName=DNS:localhost,IP:127.0.0.1"
uv run --env-file .env uvicorn stt_server.app:app --port 8000
```

The Gemini stages read `GEMINI_API_KEY` from the process environment (the
`google-genai` SDK also accepts `GOOGLE_API_KEY`). To keep the key out of the
shell history and out of git, write `GEMINI_API_KEY=...` into the gitignored
`.env` and let uv load it:

```bash
chmod 600 .env
uv run --env-file .env uvicorn stt_server.app:app --port 8000
```

`uv sync` builds `bindings/python` with maturin (Rust toolchain required).
Whisper downloads its model from Hugging Face on first use. Environment:
`MOQT_PORT` (default 4433), `MOQT_CERT`, `MOQT_KEY`. The HTTP port only serves
`GET /`; clients connect to the MoQT port.

Transcription only, no Gemini key needed:

```bash
PIPELINE_LLM=none PIPELINE_TTS=none uv run uvicorn stt_server.app:app --port 8000
```

## Browser example

`examples/browser/examples/stt` captures the microphone with getUserMedia,
encodes it to Opus with WebCodecs, publishes it over WebTransport, shows the
transcript track and plays the reply track. The server must present a
certificate the browser accepts; the repository's Chrome launcher pins the
relay certificate, so reuse it:

```bash
MOQT_PORT=4433 MOQT_CERT=../../../relay/keys/cert.pem MOQT_KEY=../../../relay/keys/key.pem \
  uv run --env-file .env uvicorn stt_server.app:app --port 8000
# in another shell, from the repository root
make browser
BROWSER_EXAMPLE_PATH=/moq-wasm/examples/stt/index.html make chrome
```

`cargo run -p relay` once generates `relay/keys/`; stop the relay before
starting the server on the same port. `make chrome` launches Chrome with
`--use-fake-device-for-media-stream`, i.e. a beeping fake microphone; drop
that flag (and `--use-fake-ui-for-media-stream`) from `scripts/chrome_mac.sh`
to use the real one.

## Test

```bash
uv sync --group dev && uv run pytest
```

The tests drive the pipeline with fake STT/LLM/TTS stages over real MoQT
sessions (both PUBLISH and PUBLISH_NAMESPACE flows), check the transcript and
reply tracks, and exercise both VADs on synthesized tones and silence.
Deepgram, OpenAI and Gemini are not called by the tests.
