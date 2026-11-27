# moq-ai-voice-agent

Browser client for speaking with the Python Pipecat AI voice agent ([`examples/python/moq-ai-voice-agent`](../../../python/moq-ai-voice-agent/README.md)) over MoQT, playing the agent's audio response and live transcript.

## Run

Quickest local setup using Docker Compose (requires a Gemini API key):

```shell
GOOGLE_API_KEY=<key> docker compose -f compose.demo.yml --profile voice-agent up   # terminal 1
MOQT_FAKE_MEDIA=0 make chrome                                                      # terminal 2
```

1. Switch access mode to **一時トークン** (temporary app) in the top right — bots launch via `services/temporary-apps` → `services/bot-dispatcher`.
2. Open `http://localhost:5173/moq-wasm/examples/moq-ai-voice-agent/`.
3. Click **Join**, then **bot を参加させる**, and speak into your microphone.

For manual bot registration without Docker Compose, see [`examples/python/moq-ai-voice-agent/README.md`](../../../python/moq-ai-voice-agent/README.md).

## Tracks

- Room namespace: `<root>/moq-ai-voice-agent/<room>` (override room with `?room=`).
- Page publishes its microphone broadcast under `<room>/<participant>`.
- Bot publishes audio and a `transcript` track under `<room>/bots/voice-agent`.
