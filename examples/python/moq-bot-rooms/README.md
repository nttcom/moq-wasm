# moq-bot-rooms

MoQ Chat Moderation・MoQ Camera Detection・MoQ PTZ Tracking の bot が共通で使う HTTP サーバーです。
ページが `GET /rooms/{room}` のイベントストリームを開いている間だけ、そのルームの bot を relay につなぎます。
設計は [AI demo bots started per room, on demand](../../../docs/architecture/ai-bots/on-demand-rooms.md) にあります。

- 同じルームを開いたページは 1 つの bot を共有します。最後のページが離れてから 30 秒たつと bot を止めます。
- `POST /rooms/{room}/leave?member={ID}` はそのページの保持をすぐ外し、`POST /rooms/{room}/stop?member={ID}` はルームの bot を全ページについて止めます。`{ID}` はストリームの最初の `member` イベントで届きます。
- ルーム ID は既定で `[a-z0-9-]` の 1〜32 文字で、namespace の 1 要素になります。
- 一時アプリのページは要求に `Authorization: Bearer <トークン>` を付けます。サーバーはトークンを token mint で更新し、その `appId` をルームの namespace root にします。トークンのない要求は `anon` のルームです。
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
| `--token-mint-url` | 環境変数 `TOKEN_MINT_URL`、なければ relay がローカルなら `http://localhost:8083`、それ以外は `https://token-mint.moqt.research.skyway.io` | 一時アプリのトークンを更新する token mint |

## テスト

```shell
uv run pytest
```
