# moq-bot-job

[bot サービス](../../../docs/architecture/bots/architecture.md)（`services/bots`）が 1 つのルームのために起動する Python bot の共通部分です。
Meeting の書き起こし、MoQ Chat Moderation、MoQ Camera Detection、MoQ PTZ Tracking、MoQ AI Voice Agent の bot が使います。

`bot_job.add_bot_job_arguments` がサービスの付ける引数（`--bot-type`、`--bot-id`、`--bots-url`、`--namespace-prefix`、`--grace-seconds`、`--max-seconds`、`--options`）とテレメトリのオプションを足し、`bot_job.serve_bot_job` がルームを動かします。

- bot は relay のトークンを `Bots.token` でサービスから受け取り、`Bots.refresh` で更新します。自分のブロードキャストは、トークンが publish を許す `<room>/bots/<type>`（`BotJob.broadcast`）に置きます。
- 5 秒ごとに heartbeat を送ります。サービスが停止を求めたら（アプリの `DELETE` か最長時間）、ルームを抜けます。
- ルームから人がいなくなって bot が抜けたとき、停止を求められたとき、SIGTERM を受けたときは `finished` を、例外で止まったときは `failed` を報告します。
- ローカルで起動された bot は環境変数 `BOTS_BOT_KEY` の鍵で、Cloud Run Job ではメタデータサーバーの ID トークンでサービスに名乗ります。

## オプション

| オプション | 既定値 | 内容 |
| --- | --- | --- |
| `--telemetry-url` | relay がローカルなら `http://localhost:8095`、それ以外は `https://observability.moqt.research.skyway.io` | `INFO` 以上のログを 10 秒ごとに送る observability サーバー |
| `--no-telemetry` | なし | ログを送らない |

bot は自分のオプションに、次の 2 つの関数で relay と ID トークンのオプションを足せます。

| 関数 | 足すオプション |
| --- | --- |
| `relay.add_relay_arguments` | `--relay-url`（既定値 `https://127.0.0.1:4433`）と、TLS 検証を省く `--insecure` |
| `identity_token.add_identity_token_arguments` | `--gcloud-auth` と `--metadata-auth`。`identity_token_from_arguments` がそれに応じた ID トークンを返す |

## テスト

```shell
uv run pytest
```
