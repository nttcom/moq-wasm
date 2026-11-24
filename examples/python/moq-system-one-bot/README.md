# moq-system-one-bot

System One（d1-2B）を使う MoQ bot（Chat Moderation / Camera Detection / PTZ Tracking）向けの共通ユーティリティです。

| モジュール | 役割 |
| --- | --- |
| `system_one_client` | `/v1/systemone` への POST（ID トークン付与、`system_one_status` 更新） |
| `event_timeline` | 判定結果を対象オブジェクトの Location に紐づける `location_record` |
| `system_one_status` | `{"system_one", "since"}` を `status` トラックへ 1 状態 1 group で書き込み |
| `relay` | 初回接続が成功するまでリトライする `stay_connected`（再接続は `moq_wasm.Session` 任せ） |
| `video` | `VideoDecoder` の出力を縦横比維持リサイズ＋JPEG 化する `picture_to_jpeg` |

## テスト

```shell
uv run pytest
```
