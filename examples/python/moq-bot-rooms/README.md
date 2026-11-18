# moq-bot-rooms

ブラウザのデモから Python bot をルーム単位で起動する共通 HTTP サーバーです。
ページが `GET /rooms/{room}` のイベントストリームを開いている間だけ、そのルームの bot を relay につなぎます。
設計は [AI demo bots started per room, on demand](../../../docs/architecture/ai-bots/on-demand-rooms.md) にあります。

- 同じルームを開いたページは 1 つの bot を共有します。最後のページとの接続が切れてから 30 秒たつと bot を止めます。音声エージェントと Meeting の書き起こし bot は、参加者のいない状態が 30 秒続いたときにも退出します。
- `POST /rooms/{room}/leave?member={ID}` はそのページの保持をすぐ外し、`POST /rooms/{room}/stop?member={ID}` はルームの bot を全ページについて止めます。`{ID}` はストリームの最初の `member` イベントで届きます。
- ルーム ID は既定で `[a-z0-9-]` の 1〜32 文字で、namespace の 1 要素になります。
- 一時アプリのページは要求に `Authorization: Bearer <トークン>` を付けます。サーバーはトークンを `temporary-apps` で更新し、その `appId` をルームの namespace root にします。トークンのない要求は `anon` のルームです。
- bot が止まった理由は、イベント名でページに届きます。

  | イベント | 送るとき |
  | --- | --- |
  | `full` | 新しいルームが `--max-rooms` を超える |
  | `closed` | `--open-hours` の時間外 |
  | `expired` | ルームが `--max-room-seconds` 動き続けた |
  | `failed` | bot が例外で止まった、またはトークンを更新できなかった |
  | `stopped` | ページが `stop` を送った |

## オプション

| オプション | 既定値 | 内容 |
| --- | --- | --- |
| `--host` / `--port` | `127.0.0.1` / bot ごと | 待ち受けるアドレス |
| `--max-rooms` | bot ごと | 同時に動かすルームの数 |
| `--max-room-seconds` | `3600` | ルームを動かし続ける最長時間（秒） |
| `--open-hours` | なし（常時） | ルームを動かす曜日と時間。例: `'Mon-Fri 09:00-17:30 +09:00'` |
| `--allowed-origin` | なし（すべて） | ルームを要求できるページの origin。複数指定できる |
| `--telemetry-url` | relay がローカルなら `http://localhost:8095`、それ以外は `https://observability.moqt.research.skyway.io` | `INFO` 以上のログを 10 秒ごとに送る observability サーバー |
| `--no-telemetry` | なし | ログを送らない |
| `--temporary-apps-url` | 環境変数 `TEMPORARY_APPS_URL`、なければ relay がローカルなら `http://localhost:8083`、それ以外は `https://token-mint.moqt.research.skyway.io` | 一時アプリのトークンを更新する temporary-apps service |

bot は自分のオプションに、次の 2 つの関数で relay と ID トークンのオプションを足せます。

| 関数 | 足すオプション |
| --- | --- |
| `relay.add_relay_arguments` | `--relay-url`（既定値 `https://127.0.0.1:4433`）と、TLS 検証を省く `--insecure` |
| `identity_token.add_identity_token_arguments` | `--gcloud-auth` と `--metadata-auth`。`identity_token_from_arguments` がそれに応じた ID トークンを返す |

## bot サービスから起動される bot

[bot サービス](../../../docs/architecture/bots/architecture.md)（`services/bots`）が 1 つのルームのために起動する bot は、`bot_job` を使います。
`add_bot_job_arguments` がサービスの付ける引数（`--bot-type`、`--bot-id`、`--bots-url`、`--namespace-prefix`、`--grace-seconds`、`--max-seconds`、`--options`）とテレメトリのオプションを足し、`serve_bot_job` がルームを動かします。

- bot は relay のトークンを `Bots.token` でサービスから受け取り、`Bots.refresh` で更新します。
- 5 秒ごとに heartbeat を送ります。サービスが停止を求めたら（アプリの `DELETE` か最長時間）、ルームを抜けます。
- ルームから人がいなくなって bot が抜けたとき、停止を求められたとき、SIGTERM を受けたときは `finished` を、例外で止まったときは `failed` を報告します。
- ローカルで起動された bot は環境変数 `BOTS_BOT_KEY` の鍵で、Cloud Run Job ではメタデータサーバーの ID トークンでサービスに名乗ります。

## テスト

```shell
uv run pytest
```
