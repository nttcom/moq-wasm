# moq-bot-job

[`bot-dispatcher`](../../../services/bot-dispatcher/README.md) が起動する Python bot 向けの共通ランナーです（transcriber / chat-moderation / camera-detection / ptz-tracking / voice-agent で共用）。

`bot_job.add_bot_job_arguments` で共通引数を定義し、`bot_job.serve_bot_job` でルーム接続・heartbeat・終了報告を処理します。

- **認証と publish**: `BotDispatcher.token` / `BotDispatcher.refresh` で relay トークンを取得・更新し、`<room>/bots/<type>`（`BotJob.broadcast`）に publish します。ローカル起動時は `BOT_DISPATCHER_BOT_KEY`、Cloud Run Job ではメタデータサーバの ID トークンで `bot-dispatcher` に認証します。
- **ライフサイクル**: 5 秒ごとに heartbeat を送り、停止要求（`DELETE` / `--max-seconds`）・無人終了・`SIGTERM` で `finished`、例外終了で `failed` を報告します。

## オプション

| オプション | 既定値 | 内容 |
| --- | --- | --- |
| `--telemetry-url` | ローカル relay: `http://localhost:8095` / それ以外: `https://observability.moqt.research.skyway.io` | `INFO` 以上を 10 秒ごとに送信する observability サーバ |
| `--no-telemetry` | なし | テレメトリ送信を無効化 |
| `--relay-url` | `https://127.0.0.1:4433` | 接続先 relay |
| `--insecure` | なし | TLS 証明書検証をスキップ |

`bot-dispatcher` が自動付与する引数: `--bot-type`, `--bot-id`, `--bot-dispatcher-url`, `--namespace-prefix`, `--grace-seconds`, `--max-seconds`, `--options`。

ID トークンが必要な bot 向けヘルパー: `identity_token.add_identity_token_arguments`（`--gcloud-auth` / `--metadata-auth`）と `identity_token_from_arguments`。

## テスト

```shell
uv run pytest
```
