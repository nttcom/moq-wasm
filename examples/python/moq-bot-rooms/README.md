# moq-bot-rooms

MoQ Chat Moderation・MoQ Camera Detection・MoQ PTZ Tracking の bot が共通で使う HTTP サーバーです。
ページが `GET /rooms/{room}` のイベントストリームを開いている間だけ、そのルームの bot を relay につなぎます。
設計は [AI demo bots started per room, on demand](../../../docs/architecture/ai-bots/on-demand-rooms.md) にあります。

- 同じルームを開いたページは 1 つの bot を共有します。最後のページが離れてから 30 秒たつと bot を止めます。
- イベントストリームは最初に `member` イベントで保持の ID を送ります。ページが `POST /rooms/{room}/leave?member={ID}` を送ると、その保持をすぐ外し、最後のページならすぐ bot を止めます。
- `POST /rooms/{room}/stop?member={ID}` は、そのルームを保持しているすべてのページについて bot を止めます。各ストリームには `stopped` が届き、次の要求は新しい bot を起動します。
- Cloud Run はページが離れてもリクエストを開いたままにするので、bot はルームのページが publish する namespace も見ています。それが 30 秒 publish されないと bot は戻り、サーバーはすぐルームを終えます。開いたままのリクエストは理由なしで終わるので、まだいるページは接続し直して新しい bot を始めます。
- ルームの bot が止まった理由は、イベント名でページに届きます。

  | イベント | 送るとき |
  | --- | --- |
  | `full` | 新しいルームが `--max-rooms` を超える |
  | `closed` | `--open-hours` の時間外に要求された、または動いている間に時間外になった |
  | `expired` | ルームが `--max-room-seconds` 動き続けた |
  | `failed` | bot が例外で止まった |
  | `stopped` | ルームを保持しているページが `stop` を送った |

- ルーム ID は既定で `[a-z0-9-]` の 1〜32 文字で、bot が `serve_rooms` に別の規則を渡せます。namespace の 1 要素になります。
- 一時アプリのページは、ルームの要求に `Authorization: Bearer <トークン>` を付けます（このためサーバーは `OPTIONS /rooms/{room}` の CORS preflight に答えます）。サーバーはそのトークンを token mint の `POST /tokens/refresh` で更新し、答えの `appId` をルームの namespace root にします。issuer が拒んだら 401、答えなければ 503 を返し、bot は起動しません。トークンのない要求は今までどおり `anon` のルームです。ルームは namespace root とルーム ID の組で区別します。
- bot は自分のトークンで relay につなぎ、期限の 5 分前に issuer で更新して、開いているセッションに `Session.refresh_token` で渡します。issuer に拒まれたとき、または期限まで答えがなかったときは、ルームを `failed` で終えます。設計は [Temporary apps](../../../docs/architecture/ai-bots/on-demand-rooms.md#temporary-apps) にあります。
- bot が `serve_rooms` に `bot_from_query` を渡すと、ルームを始めたリクエストのクエリでそのルームの bot を選べます（例: 書き起こしのモデル）。受け付けないクエリには 400 を返します。

## オプション

| オプション | 既定値 | 内容 |
| --- | --- | --- |
| `--host` / `--port` | `127.0.0.1` / bot ごと | 待ち受けるアドレス |
| `--max-rooms` | bot ごと | 同時に動かすルームの数 |
| `--max-room-seconds` | `3600` | ルームを動かし続ける最長時間（秒） |
| `--open-hours` | なし（常時） | ルームを動かす曜日と時間。例: `'Mon-Fri 09:00-17:30 +09:00'` |
| `--allowed-origin` | なし（すべて） | ルームを要求できるページの origin。複数指定できる |
| `--telemetry-url` | relay がローカルなら `http://localhost:8095`、それ以外は `https://observability.moqt.research.skyway.io` | ログを送る observability サーバー |
| `--no-telemetry` | なし | ログを送らない |
| `--token-mint-url` | 環境変数 `TOKEN_MINT_URL`、なければ relay がローカルなら `http://localhost:8083`、それ以外は `https://token-mint.moqt.research.skyway.io` | 一時アプリのトークンを更新する token mint |

## テレメトリー

bot の `INFO` 以上のログ（loguru）を 10 秒ごとと終了時に observability サーバーの `/api/client-telemetry` へ送ります。HyperDX では `service.name=python`、`page` が bot の名前（例: `moq-camera-detection`）になります。送信に失敗したログと、10 秒間に 100 件を超えた古いログは捨てます。

## テスト

```shell
uv run pytest
```
