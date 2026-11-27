# live-translate

Watches live MSF broadcasts on a MoQT relay and publishes on-demand translated Opus audio (`audio_en`, `audio_es`, `audio_de`, `audio_ko`, `audio_zh-Hans`) and live transcripts (`text_<lang>`, `text_ja`) under `<root>/<id>/translated` using Gemini Live Translate. Design: [architecture.md](../../docs/architecture/live-translate/architecture.md).

## Run

Local dev without Google Cloud (`--echo` returns delayed source audio):

```shell
make relay
cargo run -p live-translate -- --relay moqt://127.0.0.1:4433 --insecure --public-root --echo
```

With Gemini Live Translate (impersonating a service account locally; on Cloud Run the metadata server is used automatically):

```shell
cargo run -p live-translate -- \
  --relay moqt://127.0.0.1:4433 --insecure --public-root \
  --gcp-project <project-id> --gcloud-impersonate <service-account-email>
```

Open [`examples/browser/examples/live-translate/`](../../examples/browser/examples/live-translate/README.md) to broadcast and listen to translated tracks.

## Options

| Flag / Variable | Default | Meaning |
| --- | --- | --- |
| `--relay` (`LIVE_TRANSLATE_RELAY_URL`) | `moqt://localhost:4433` | Target relay (`moqt://`, `https://`, or `wss://`) |
| `--insecure` | off | Skip TLS certificate verification (local dev only) |
| `--http-port` (`PORT`) | — | Serve `POST /sessions`, `GET /events`, and `GET /healthz` for temporary apps |
| `--public-root` | off | Also watch `anon/live-translate` without a token |
| `--echo` | off | Loop source audio with per-language delay instead of calling Gemini |
| `--gcp-project` (`LIVE_TRANSLATE_GCP_PROJECT`) | — | GCP project serving Gemini Enterprise Agent Platform (required unless `--echo`) |
| `--gcp-location` (`LIVE_TRANSLATE_GCP_LOCATION`) | `global` | Agent Platform location |
| `--gcloud-impersonate` (`LIVE_TRANSLATE_GCLOUD_IMPERSONATE`) | — | Service account email to impersonate via `gcloud` outside GCP |
| `--max-sessions` (`LIVE_TRANSLATE_MAX_SESSIONS`) | `5` | Max concurrent language translations across all broadcasts |
| `--max-apps` (`LIVE_TRANSLATE_MAX_APPS`) | `5` | Max concurrent temporary-app relay sessions |
| `--language-time-limit-minutes` (`LIVE_TRANSLATE_LANGUAGE_TIME_LIMIT_MINUTES`) | `60` | Max continuous translation minutes per on-demand language |
| `--always-on` (`LIVE_TRANSLATE_ALWAYS_ON`) | — | Comma-separated language codes (e.g. `en,ko`) kept running for the whole broadcast |

Startup requires `--public-root` or `PORT` (or both), and `--echo` or `LIVE_TRANSLATE_GCP_PROJECT`.

## HTTP API (`PORT`)

| Route | Purpose |
| --- | --- |
| `POST /sessions` (`Authorization: Bearer <token>`) | Register a temporary app and watch `<appId>/live-translate` |
| `GET /events` | SSE keepalive stream held by broadcaster pages |
| `GET /healthz` | Health check |

## E2E

```shell
./tests/live-translate-e2e/run.sh
make browser-e2e-live-translate
```
