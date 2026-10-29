# moq-system-one-bot

MoQ Chat Moderation・MoQ Camera Detection・MoQ PTZ Tracking の bot が共通で使う、d3-flash を呼ぶ MoQ bot の部品です。

| モジュール | 内容 |
| --- | --- |
| `system_one_status` | d3-flash の状態 `{"system_one", "since"}` を status トラックに書く。1 状態 1 group |
| `identity_token` | `--gcloud-auth` と `--metadata-auth` のオプションと、それに応じた ID トークン（`moq-bot-rooms` の `identity_token`） |
| `namespace` | ページの namespace が publish されている間だけ処理を動かす `while_published` |
| `publication` | bot が書くトラックの型（`moqt.TrackPublication` とテストの代役） |

## テスト

```shell
uv run pytest
```
