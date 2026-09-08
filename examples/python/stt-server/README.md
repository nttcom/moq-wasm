# MoQT speech-to-text server (FastAPI)

A FastAPI process that listens for MoQT sessions, decodes the audio tracks it
receives to PCM16 and streams them into a speech-to-text backend. Transcripts
are logged and exposed at `GET /transcripts`; `GET /` reports counters.

## How audio reaches the server

- **PUBLISH** (moq-cli, bridges/live-ingest, the Python bindings): every
  track is accepted. `catalog` objects are parsed for audio tracks; a track
  named in the catalog with `role: audio` / an audio codec is transcribed, as
  is a track named `AUDIO_TRACK_NAME` (default `audio`) when no catalog
  arrived.
- **PUBLISH_NAMESPACE** (the browser call example): the server subscribes to
  `<namespace>/catalog`, then to every audio track the catalog lists.

Object payloads may be bare `EncodedAudioChunk` bytes (browser, LOC) or the
`bridges/live-ingest` framing (4-byte length + JSON metadata + frame); the
metadata's codec wins over the catalog. Opus and AAC (`mp4a.40.x`) are
decoded with PyAV and resampled to the backend's PCM format.

## Backends

| `STT_BACKEND` | Service | PCM sent | Credentials |
| --- | --- | --- | --- |
| `wav` (default) | none: writes `TRANSCRIPT_DIR/<track>.wav` | 16 kHz mono | – |
| `deepgram` | Deepgram live API (`nova-3`) | 16 kHz mono linear16 | `DEEPGRAM_API_KEY` |
| `openai` | OpenAI Realtime transcription (`gpt-4o-transcribe`) | 24 kHz mono pcm16 | `OPENAI_API_KEY` |
| `whisper` | faster-whisper on this machine (`uv sync --group whisper`) | 16 kHz mono, per utterance | – |

`STT_LANGUAGE` (default `ja`) is passed to the service. Whisper is a batch
model: audio is cut into utterances at pauses (`SpeechSegmenter`, at most
`WHISPER_MAX_SEGMENT_SEC`, default 10 s) and each utterance is transcribed on
a worker thread. `WHISPER_MODEL` (default `small`), `WHISPER_DEVICE` (`cpu`)
and `WHISPER_COMPUTE_TYPE` (`int8`) select the model; the first run downloads
it from Hugging Face. A new service is one
class implementing `stt_server.stt.base.SpeechToText` (`pcm_format`,
`start`, `send_pcm`, `close`) plus a branch in `create_backend`.

## Run

```bash
cd examples/python/stt-server
uv sync
openssl req -x509 -nodes -days 1 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
  -keyout key.pem -out cert.pem -subj /CN=localhost \
  -addext "subjectAltName=DNS:localhost,IP:127.0.0.1"
STT_BACKEND=deepgram DEEPGRAM_API_KEY=... uv run uvicorn stt_server.app:app --port 8000
```

Environment: `MOQT_PORT` (default 4433), `MOQT_CERT`, `MOQT_KEY`.

## Test

```bash
uv sync --group dev && uv run pytest
```

The tests publish an Opus sine wave through the Python bindings, over both
the PUBLISH and PUBLISH_NAMESPACE flows, and check the decoded PCM that the
`wav` backend received.
