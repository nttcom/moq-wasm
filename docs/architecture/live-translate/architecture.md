# `live-translate` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
`live-translate` watches `anon/live-translate` on a relay, and for every broadcast
`anon/live-translate/{id}` publishes `anon/live-translate/{id}/translated`: an MSF catalog
that references the broadcast's own video and audio, plus one Opus track per
target language (`audio_en`, `audio_es`, `audio_de`, `audio_ko`,
`audio_zh-Hans`). A language is translated by Gemini Live Translate only while
someone subscribes to its track. Gemini is called on the Gemini Enterprise
Agent Platform (`gemini/platform.rs`): the `global` location's Live API over a
WebSocket with `Authorization: Bearer`, as the service account the Cloud Run
runs as (`metadata-token`) or, outside Google Cloud, as that account
impersonated through `gcloud` per connection
(`LIVE_TRANSLATE_GCLOUD_IMPERSONATE`). `--echo` returns the
source audio after each language's fixed delay instead of calling Gemini, which
keeps the whole path testable without Google Cloud; without either `--echo` or
`LIVE_TRANSLATE_GCP_PROJECT` the translator refuses to start.

Three limits bound the Gemini cost besides the `--max-sessions` cap
(`translation_limits.rs`): a language is translated only inside
`--open-hours` (the bots' notation, `Mon-Fri 09:00-17:30 +09:00`; a SUBSCRIBE
outside them gets SUBSCRIBE_ERROR), for at most
`--language-time-limit-minutes` at a stretch, and the `BroadcastTranslation`
checks both every minute, ending the language's subscriptions with
PUBLISH_DONE so the viewers fall back to the original audio and may subscribe
again.

## Runtime flow

```
main (Router)                one task; the only reader of Session::receive_event
 ├─ PUBLISH_NAMESPACE        direct children of anon/live-translate start a BroadcastTranslation
 ├─ PUBLISH_NAMESPACE_DONE   drops it
 ├─ SUBSCRIBE                routed to the BroadcastTranslation by namespace
 ├─ UNSUBSCRIBE              offered to every BroadcastTranslation; the one holding the request id acts
 └─ BroadcastTranslation     one actor per broadcast
     ├─ SourceCatalog              SUBSCRIBE {id}/catalog (joining FETCH if the relay already caches it)
     │                             and follow it: the browser publisher lists each track once its encoder runs
     ├─ PUBLISH_NAMESPACE .../translated
     ├─ CatalogPublication         one per catalog subscriber; re-sends the catalog every 10 s and
     │                             whenever a source catalog changes the translated one
     ├─ SourceAudio                SUBSCRIBE the highest-bitrate Opus track, decode to 16 kHz,
     │                             broadcast (latest input timestamp, PCM) to the languages
     └─ LanguageTranslation        one per subscribed language
         ├─ RunningTranslation     Gemini WebSocket session or echo, PCM in / PCM out
         ├─ TranslatedAudioSequencer   timestamps (see below)
         └─ OpusEncoder → TrackWriter  20 ms objects, LOC capture timestamp, group = ts / 2 s
```

The relay forwards a SUBSCRIBE only for a track's first downstream subscriber and
an UNSUBSCRIBE only when the last one leaves, but only for subscriptions it
created with SUBSCRIBE. That is why the translated tracks are served on demand
(PUBLISH_NAMESPACE, then answer SUBSCRIBE) and never pushed with PUBLISH.

## Timestamps

Every LOC capture timestamp means "when this was said", on the broadcaster's
clock. Gemini Live Translate returns 250 ms chunks at real-time pace for as long
as the connection lasts, silence included, so within one connection the length
of the translated audio is the elapsed time.

- The *input cursor* is the capture timestamp of the newest source object pushed
  to the decoder. It stands in for "now" on the broadcaster's clock, so the
  translator never reads its own clock for timestamps.
- When a Gemini connection starts (the first one and every reconnect), the next
  translated frame is stamped input cursor − the language's fixed Gemini delay.
- Every following frame is stamped 20 ms after the previous one, even across a
  delivery stall: the audio after a stall continues the same stream.

Gemini does not say which input a chunk translates, so the delay is a fixed
per-language estimate (`target_language.rs`); the error per utterance is the
difference between the actual and the estimated delay.

## Invariants

- Exactly one task calls `Session::receive_event`.
- A `LanguageTranslation` owns its `TrackWriter`; after the last viewer leaves
  it keeps the Gemini session for 5 s and resumes with a new writer if a
  SUBSCRIBE returns in time.
- At most `--max-sessions` languages are translated at once across all
  broadcasts (the Gemini free tier allows 3 concurrent sessions); a SUBSCRIBE
  beyond that is rejected.
- Translated timestamps never decrease, so group ids never decrease.

## Testing conventions
- Timestamp, framing, catalog and Gemini message logic are unit-tested without
  I/O. The Opus pipelines are tested against the local GStreamer install.
- `tests/live-translate-e2e/run.sh` runs a fake broadcaster and viewer against a
  relay with the translator in `--echo` mode and checks that translated timestamps
  land on the source time.
