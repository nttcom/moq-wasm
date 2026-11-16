# moq-system-one-bot

MoQ Chat Moderation・MoQ Camera Detection・MoQ PTZ Tracking の bot が共通で使う、System One（d1-2B）を呼ぶ MoQ bot の部品です。

| モジュール | 内容 |
| --- | --- |
| `system_one_client` | System One の `/v1/systemone` への POST。ID トークンを付け、待ち時間を `system_one_status` に反映する |
| `event_timeline` | bot の判定を、判定した object の Location で引ける event timeline の record にする `location_record`（トラックは `moq_wasm.media` の `EventTimeline`） |
| `system_one_status` | System One の状態 `{"system_one", "since"}` を status トラックに書く。1 状態 1 group |
| `identity_token` | `--gcloud-auth` と `--metadata-auth` のオプションと、それに応じた ID トークン（`moq-bot-rooms` の `identity_token`） |
| `relay` | `--relay-url` と `--insecure` のオプションと、最初の接続がつながるまで試し続ける `stay_connected`（つなぎ直しは `moq_wasm.Session` が行う） |
| `video` | `moq_wasm.codecs` の `VideoDecoder` で復号した絵を JPEG にする `picture_to_jpeg`（幅を指定すると縦横比を保って縮小する） |
| `publication` | bot が書くトラックの型（`moq_wasm.TrackWriter` とテストの代役） |

## テスト

```shell
uv run pytest
```
