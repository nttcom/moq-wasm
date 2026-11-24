# live-translate

Streams camera/mic from a broadcaster while viewers pick a language translated live by `services/live-translate`.

## Run locally

```shell
make relay
cargo run -p live-translate -- --relay moqt://127.0.0.1:4433 --insecure --public-root --echo   # --echo loops source audio with delay (no Gemini)
make browser
```

Open `examples/live-translate/streamer/index.html` (`MOQT_FAKE_MEDIA=0 make chrome` for real devices), click **配信開始**, and open the viewer link shown.

For real translation, replace `--echo` with `--gcp-project <project id> --gcloud-impersonate <service account email>` (the logged-in `gcloud` user needs `roles/iam.serviceAccountTokenCreator` on that account).

| Translator option               | Effect                                                          |
| ------------------------------- | --------------------------------------------------------------- |
| `--language-time-limit-minutes` | Max continuous translation time per language (default: `60`)    |
| `--always-on en,ko`             | Keep listed languages active throughout so switching takes ~1 s |

Viewers play 5 s behind capture (`?delay=4.5` to override) with captions for the active language. The Stats line shows delay, A/V offset, and switch time (also sent to page telemetry every 10 s).

## Access

Follows the example hub access mode ([`examples/browser/README.md`](../../README.md)):

- **Public**: broadcasts under `anon/live-translate/<id>`, auto-watched by `--public-root`.
- **Temporary app**: broadcasts under `<appId>/live-translate/<id>`; the streamer passes an app token via `POST /sessions` (up to 1 h). Locally, run `docker compose up -d vts temporary-apps` and start the translator with `PORT=18090` (override URL with `?liveTranslateUrl=`).

## Tips

Feed a looping 16-bit PCM WAV to Chrome's fake mic instead of hearing your own echo 5 s later:

```shell
say -v Kyoko -o /tmp/speech.aiff "こんにちは。今日はリアルタイム翻訳のデモをお見せします。"
afconvert -f WAVE -d LEI16@48000 -c 1 /tmp/speech.aiff /tmp/speech.wav
MOQT_FAKE_AUDIO_FILE=/tmp/speech.wav make chrome
```

Without an H.264 encoder, append `?videoCodec=vp8&videoEncoderHardwareAcceleration=prefer-software` to the streamer URL.
