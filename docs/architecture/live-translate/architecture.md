# `live-translate` Architecture

## Status
Living document. Update alongside any change to design intent, module boundaries, runtime flow, or invariants.

## Scope
`live-translate` watches a root namespace on a relay and, for each broadcast `{root}/{id}`, publishes `{root}/{id}/translated` with:
- **MSF catalog**: References the source video/audio plus translated audio and transcript tracks.
- **Translated audio**: One Opus track per target language (`audio_en`, `audio_es`, `audio_de`, `audio_ko`, `audio_zh-Hans`).
- **Transcripts**: `text_<lang>` (Gemini output, stamped like translated audio) and `text_ja` (source speech heard by Gemini, stamped with newest source audio time; written by one active language and refused when none runs), each as JSON `{"text", "final"}`.

### Translation Backends & Limits
- **Gemini Live Translate** (`gemini/platform.rs`): Connects to Gemini Enterprise Agent Platform (`global` Live API over WebSocket with `Authorization: Bearer`) using `google-cloud`'s `MetadataToken` on Cloud Run, or `gcloud` impersonation (`LIVE_TRANSLATE_GCLOUD_IMPERSONATE`) locally.
- **`--echo` mode**: Returns source audio after each language's fixed delay without Google Cloud. Startup fails unless `--echo` or `LIVE_TRANSLATE_GCP_PROJECT` is set.
- **On-demand vs. `--always-on`** (`TranslationPolicy` in `root_translation.rs`): Languages translate only while subscribed, except `--always-on` languages which run for the whole broadcast (~1 s tune-in instead of waiting for Gemini's first response) and ignore viewer count and time limits.
- **Time & failure caps**: Non-always-on languages stop after `--language-time-limit-minutes` (checked every minute, ending subscriptions with `PUBLISH_DONE` so viewers fall back to original audio). A language also ends with `translation unavailable` if Gemini closes with a policy violation or rapid failures back off to 30 s.

### HTTP Server & Temporary Apps (`session_server.rs`, `temporary_apps.rs`)
When `PORT` is set, the HTTP server binds before dialing the relay (all responses allow any CORS origin):

| Route | Purpose |
| --- | --- |
| `POST /sessions` (`Authorization: Bearer <token>`) | Registers a temporary app (`services/temporary-apps`) and opens a relay session watching `{appId}/live-translate`. Re-posting for an active app restarts it with the new token. Max `--max-apps` (5) apps at once; an app idle for 1 minute (or with no broadcast) is evicted. |
| `GET /events` | SSE keepalive (comment every 15 s) held by streamer pages so Cloud Run stays alive while broadcasting and scales to zero when idle. |
| `GET /healthz` | Health check. |

With `--public-root` (local dev and E2E), the service also watches `anon/live-translate` without a token. Tokens are never refreshed; the relay closes the session at expiry (≤ 1 hour), ending translation. Callers are unauthenticated; cost is bounded by session/time limits and project Gemini billing caps.

## Runtime Flow

```mermaid
flowchart TB
    SS["SessionServer (/sessions, /events, /healthz)"] --> TA["TemporaryApps (per-app AppTranslation)"]
    TA --> RT["RootTranslation (1 relay session, sole Session::receive_event reader)"]
    RT -->|"PUBLISH_NAMESPACE / DONE / SUBSCRIBE / UNSUBSCRIBE"| BT["BroadcastTranslation (per broadcast)"]
    BT --> SC["SourceCatalog (SUBSCRIBE + joining FETCH)"]
    BT --> CP["CatalogTrack (re-sends every 10 s + on change)"]
    BT --> SA["SourceAudio (highest-bitrate Opus → 16 kHz PCM)"]
    SA --> LT["LanguageTranslation (per active language)"]
    LT --> Gem["RunningTranslation (Gemini WS or --echo)"]
    Gem --> Seq["TranslatedAudioSequencer → OpusEncoder → TrackWriter"]
```

- **Catalog & source audio**: `SourceCatalog` follows `{id}/catalog` dynamically as browser encoders come online. `SourceAudio` subscribes to the highest-bitrate Opus track (`DELIVERY TIMEOUT` 5 s), decodes to 16 kHz PCM, broadcasts `(latest_input_ts, PCM)` to active languages, and resubscribes 1 s after close/error.
- **Why `PUBLISH_NAMESPACE` instead of `PUBLISH`**: The relay forwards `SUBSCRIBE` only for the first downstream viewer and `UNSUBSCRIBE` when the last leaves—only for `SUBSCRIBE`-initiated tracks. Using `PUBLISH_NAMESPACE` keeps translation strictly on demand.
- **Output framing**: `OpusEncoder → TrackWriter` emits 20 ms objects with LOC capture timestamps and `group = ts / 2 s`.

## Timestamps
LOC capture timestamps record "when this was said" on the broadcaster's clock (the translator never reads its own wall clock). Gemini outputs 250 ms real-time chunks continuously (including silence):
- **Input cursor**: Capture timestamp of the newest source audio object pushed to the decoder.
- **Connection anchor**: On initial connect and every reconnect, the first translated frame is stamped `input_cursor - fixed_gemini_delay` (per-language estimate in `target_language.rs`, since Gemini does not identify which input chunk produced an output).
- **Monotonic stepping**: Every subsequent frame advances by +20 ms even across delivery stalls.

## Invariants

**Session & auth**
- **Single event reader**: Exactly one task calls `Session::receive_event` per session.
- **Transport agnostic**: `RootTranslation` uses `Endpoint<NATIVE>` (`moqt://`, `https://`, or `wss://`).
- **No local token verification**: Temporary app tokens are passed straight to the relay (which rejects invalid ones) and never logged. No relay secrets are stored.

**Resource & timing caps**
- **Idle grace & `text_ja` isolation**: `LanguageTranslation` owns its `TrackWriter` and keeps Gemini open for 5 s after the last viewer leaves. `text_ja` lives outside per-language subscriber tracking so transcript-only viewers never keep a language alive; it ends with its writer language.
- **Global concurrency cap**: At most `--max-sessions` (5) languages run across all broadcasts and roots; excess `SUBSCRIBE`s are rejected immediately rather than queuing against project quota.
- **Monotonic timestamps**: Translated timestamps and group IDs never decrease.

## Testing Conventions
- Timestamp, framing, catalog, and Gemini message logic are unit-tested without I/O; Opus pipelines test against local GStreamer.
- `tests/live-translate-e2e/run.sh` runs a fake broadcaster and viewer against a relay in `--echo` mode and verifies translated timestamps align with source time.
