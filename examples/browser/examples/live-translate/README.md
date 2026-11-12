# live-translate

A broadcaster streams camera and microphone; viewers pick a language and hear the broadcast translated by `services/live-translate`.

## Run locally

```shell
make relay
cargo run -p live-translate -- --relay moqt://127.0.0.1:4433 --insecure --echo   # --echo returns the source audio after each language's delay, without Gemini
make browser
```

To translate for real, name the project and the service account; `gcloud`, logged in as a user with `roles/iam.serviceAccountTokenCreator` on that account, borrows its token for each connection:

```shell
cargo run -p live-translate -- --relay moqt://127.0.0.1:4433 --insecure --gcp-project <project id> --gcloud-impersonate <service account email>
```

A language is translated for at most 60 minutes at a stretch (`--language-time-limit-minutes`) and, with `--open-hours 'Mon-Fri 09:00-17:30 +09:00'`, only inside those hours; the viewer returns to the original audio when the translation ends. `--always-on en,ko` keeps those languages translating for the whole broadcast, so switching to them takes about a second instead of waiting for Gemini's first answer.

The viewer shows captions on the picture: the transcript of the language heard (`text_<lang>`), or of the Japanese (`text_ja`) while some language is being translated. The viewer plays every track 5 s after capture; `?delay=4.5` on its URL tries another delay. Its Stats line shows the delay, the A/V offset, how long the last switch took and the samples dropped late; every 10 s the same numbers go to the page telemetry (`viewer stats` in HyperDX, or in ClickHouse's `otel.otel_logs` with `docker compose up -d observability` locally).

Open `examples/live-translate/streamer/index.html` (`MOQT_FAKE_MEDIA=0 make chrome` for the real camera and microphone), press 配信開始, and open the viewer link it shows.

## Access

The pages follow the access mode of the example list (`utils/access.ts`).

- Public (the default on this machine): the broadcast is `anon/live-translate/<id>`, which the translator watches by itself.
- Temporary app (the default when deployed): the broadcast is `<appId>/live-translate/<id>`. The translator reaches an app only with one of its tokens, so the streamer page hands it a newly signed one (`POST /sessions`) and holds its event stream (`GET /events`) while it broadcasts. The translator keeps that token until it expires, so a broadcast is translated for an hour at most; start it again for another hour. The viewer link carries the token, and opening it joins the app.

To try a temporary app locally, run the token mint and the VTS (`docker compose up -d vts token-mint`), start the translator with `PORT=18090`, and choose the temporary mode on the example list. `?liveTranslateUrl=` points the streamer at another translator.

To judge a translation without hearing your own voice five seconds later, let the fake microphone play prepared speech instead (a 16-bit PCM WAV, looped):

```shell
say -v Kyoko -o /tmp/speech.aiff "こんにちは。今日はリアルタイム翻訳のデモをお見せします。"
afconvert -f WAVE -d LEI16@48000 -c 1 /tmp/speech.aiff /tmp/speech.wav
MOQT_FAKE_AUDIO_FILE=/tmp/speech.wav make chrome
```

In a browser without an H.264 encoder add `?videoCodec=vp8&videoEncoderHardwareAcceleration=prefer-software` to the streamer URL.
