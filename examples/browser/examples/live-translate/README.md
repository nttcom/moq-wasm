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

A language is translated for at most 60 minutes at a stretch (`--language-time-limit-minutes`) and, with `--open-hours 'Mon-Fri 09:00-17:30 +09:00'`, only inside those hours; the viewer returns to the original audio when the translation ends.

Open `examples/live-translate/streamer/index.html`, press 配信開始, and open the viewer link it shows. In a browser without an H.264 encoder add `?videoCodec=vp8&videoEncoderHardwareAcceleration=prefer-software` to the streamer URL.
