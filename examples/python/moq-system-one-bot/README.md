# moq-system-one-bot

MoQ Chat Moderation・MoQ Camera Detection・MoQ PTZ Tracking の bot が共通で使う、System One（d1-2B）を呼ぶ MoQ bot の部品です。

| モジュール | 内容 |
| --- | --- |
| `system_one_client` | System One の `/v1/systemone` への POST。ID トークンを付け、待ち時間を `system_one_status` に反映する |
| `event_timeline` | bot の判定を書く MSF event timeline トラック |
| `system_one_status` | System One の状態 `{"system_one", "since"}` を status トラックに書く。1 状態 1 group |
| `identity_token` | `--gcloud-auth` と `--metadata-auth` のオプションと、それに応じた ID トークン（`moq-bot-rooms` の `identity_token`） |
| `relay` | `--relay-url` と `--insecure` のオプションと、relay への接続が切れるたびにつなぎ直す `stay_connected` |
| `namespace` | ページの namespace が publish されている間だけ処理を動かす `while_published` |
| `publication` | bot が書くトラックの型（`moqt.TrackPublication` とテストの代役） |

## テスト

```shell
uv run pytest
```
