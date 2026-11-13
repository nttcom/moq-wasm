# live-translate

A broadcaster streams camera and microphone; viewers pick a language and hear the broadcast translated by `services/live-translate`.

## Run locally

```shell
make relay
cargo run -p live-translate -- --relay moqt://127.0.0.1:4433 --insecure --public-root --echo   # --echo returns the source audio after each language's delay, without Gemini
make browser
```

Open `examples/live-translate/streamer/index.html` (`MOQT_FAKE_MEDIA=0 make chrome` for the real camera and microphone), press 配信開始, and open the viewer link it shows.

To translate for real, replace `--echo` with `--gcp-project <project id> --gcloud-impersonate <service account email>`; `gcloud` must be logged in as a user with `roles/iam.serviceAccountTokenCreator` on that account.

| Translator option                           | Effect                                                                                              |
| ------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `--language-time-limit-minutes`             | How long a language is translated at a stretch (60 by default)                                      |
| `--open-hours 'Mon-Fri 09:00-17:30 +09:00'` | Translate only inside those hours                                                                   |
| `--always-on en,ko`                         | Keep those languages translating for the whole broadcast, so switching to them takes about a second |

The viewer plays every track 5 s after capture (`?delay=4.5` tries another delay) and shows the transcript of the language heard as captions. Its Stats line shows the delay, the A/V offset and the switch time; the same numbers go to the page telemetry every 10 s.

## Access

The pages follow the access mode of the example list (see [`examples/browser/README.md`](../../README.md)).

- **Public**: the broadcast is `anon/live-translate/<id>`, which a translator started with `--public-root` watches by itself.
- **Temporary app**: the broadcast is `<appId>/live-translate/<id>`. The streamer page hands the translator a token of the app (`POST /sessions`), so a broadcast is translated for an hour at most. To try it locally, run `docker compose up -d vts token-mint` and start the translator with `PORT=18090`; `?liveTranslateUrl=` points the streamer at another translator.

## Tips

Let the fake microphone play prepared speech (a 16-bit PCM WAV, looped) instead of hearing your own voice five seconds later:

```shell
say -v Kyoko -o /tmp/speech.aiff "こんにちは。今日はリアルタイム翻訳のデモをお見せします。"
afconvert -f WAVE -d LEI16@48000 -c 1 /tmp/speech.aiff /tmp/speech.wav
MOQT_FAKE_AUDIO_FILE=/tmp/speech.wav make chrome
```

In a browser without an H.264 encoder add `?videoCodec=vp8&videoEncoderHardwareAcceleration=prefer-software` to the streamer URL.
