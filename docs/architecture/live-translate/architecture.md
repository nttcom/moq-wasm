# `live-translate` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
`live-translate` watches a root namespace on a relay, and for every broadcast
`{root}/{id}` publishes `{root}/{id}/translated`: an MSF catalog
that references the broadcast's own video and audio, plus one Opus track per
target language (`audio_en`, `audio_es`, `audio_de`, `audio_ko`,
`audio_zh-Hans`), and the transcripts as `text_<lang>` (what Gemini said) and
`text_ja` (what it heard), one JSON object `{"text", "final"}` per piece,
stamped like the translated audio so a viewer shows each piece when that
audio plays; `text_ja` is written by one running language and refused while
none runs. A language is translated by Gemini Live Translate only while
someone subscribes to its track. Gemini is called on the Gemini Enterprise
Agent Platform (`gemini/platform.rs`): the `global` location's Live API over a
WebSocket with `Authorization: Bearer`, as the service account the Cloud Run
runs as (`metadata-token`) or, outside Google Cloud, as that account
impersonated through `gcloud` per connection
(`LIVE_TRANSLATE_GCLOUD_IMPERSONATE`). `--echo` returns the
source audio after each language's fixed delay instead of calling Gemini, which
keeps the whole path testable without Google Cloud; without either `--echo` or
`LIVE_TRANSLATE_GCP_PROJECT` the translator refuses to start.

`--always-on` languages (`translation_policy.rs`) start with the broadcast
and run while it lasts, whoever listens, so their first viewer hears them
about a second after choosing them instead of waiting for Gemini's first
answer. Three limits bound the Gemini cost besides the `--max-sessions`
cap: a language is translated only inside
`--open-hours` (the bots' notation, `Mon-Fri 09:00-17:30 +09:00`; a SUBSCRIBE
outside them gets SUBSCRIBE_ERROR), for at most
`--language-time-limit-minutes` at a stretch, and the `BroadcastTranslation`
checks both every minute, ending the language's subscriptions with
PUBLISH_DONE so the viewers fall back to the original audio and may subscribe
again. An always-on language ignores the time limit and viewers leaving, and
restarts when the open hours resume. A language also ends, with the reason
`translation unavailable`, when its Gemini session gives up: Gemini closed the
connection with a policy violation (the API disabled, the account refused), or
quick failures in a row have backed off all the way to 30 s. Reconnecting
cannot mend either, and a viewer waiting on it would otherwise wait forever.

With `PORT` set the process also serves the requests of the streamers' pages
(`session_server.rs`): `POST /sessions`, below, and `GET /events`, a
server-sent event stream that only sends a comment every 15 s. A streamer's
page holds `/events` while it broadcasts: a Cloud Run instance lives while a
request is in flight, so that stream is what keeps the translator running, and
lets the service scale to zero when every page is closed. Outside the open
hours the stream ends at its next heartbeat and a new one gets 204, which
stops an `EventSource` from reconnecting, so a tab left open does not keep the
instance up past closing time. The service does not identify its callers.
What a caller can make the translation cost is bounded instead, by the limits
above and by the deployment's limit on what the Gemini API may be billed.
The server listens before the relay is dialed, so the port is open as soon as
the container is.

`POST /sessions` with `Authorization: Bearer <token>` has the translator
serve a temporary app (`services/token-mint`), the namespace root the
examples run under (`temporary_apps.rs`). An app is reached only with its own
tokens, so the streamer's page hands one over and the translator opens a
relay session of its own with it, watching `{appId}/live-translate` as the
process watches the public `anon/live-translate` by itself. With
`--temporary-apps-only`, as deployed, it watches no public root. Nobody renews the token: the relay
closes the session when it expires, an hour after it was signed at most,
which ends the app's translation. A request for an app already served starts
it over with the new token. At most `--max-apps` apps (5) are served at once;
one whose last broadcast ended a minute ago, or that never had one, gives its
place up. Every response allows any origin, since those pages are served from
another one.

## Runtime flow

```
SessionServer                one http_server::HttpServer; /sessions, /events, /healthz
 └─ TemporaryApps            POST /sessions: one AppTranslation task per temporary app,
                             each running a RootTranslation of its own
RootTranslation              one relay session; the only reader of Session::receive_event
 ├─ PUBLISH_NAMESPACE        direct children of the root start a BroadcastTranslation
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
     │                             broadcast (latest input timestamp, PCM) to the languages;
     │                             subscribes again a second after the subscription ends or fails
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

- Exactly one task calls `Session::receive_event` of a session.
- The translator never verifies a temporary app's token: the relay refuses
  the session of one it cannot verify, and the token is kept only in that
  session's configuration, never logged.
- A `LanguageTranslation` owns its `TrackWriter`; after the last viewer leaves
  it keeps the Gemini session for 5 s and resumes with a new writer if a
  SUBSCRIBE returns in time. The `text_ja` subscription is held beside the
  languages, not among a language's own, so a transcript viewer never keeps a
  language from idling; it ends with the language that writes it.
- At most `--max-sessions` languages (5) are translated at once across all
  broadcasts of every root, so the Gemini cost has a ceiling; a SUBSCRIBE beyond that is
  rejected at once rather than left waiting on a connection the project's
  quota, set to the same number, would refuse.
- The translator holds no secret of the relay's: the public root needs no
  token, and a temporary app is reached with the token its streamer hands
  over.
- Translated timestamps never decrease, so group ids never decrease.

## Testing conventions
- Timestamp, framing, catalog and Gemini message logic are unit-tested without
  I/O. The Opus pipelines are tested against the local GStreamer install.
- `tests/live-translate-e2e/run.sh` runs a fake broadcaster and viewer against a
  relay with the translator in `--echo` mode and checks that translated timestamps
  land on the source time.
