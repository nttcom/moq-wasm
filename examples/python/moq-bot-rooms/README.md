# moq-bot-rooms

MoQ Chat Moderation・MoQ Camera Detection・MoQ PTZ Tracking の bot が共通で使う HTTP サーバーです。
ページが `GET /rooms/{room}` のイベントストリームを開いている間だけ、そのルームの bot を relay につなぎます。
設計は [AI demo bots started per room, on demand](../../../docs/architecture/ai-bots/on-demand-rooms.md) にあります。

- 同じルームを開いたページは 1 つの bot を共有します。最後のページが離れてから 30 秒たつと bot を止めます。
- ルームの bot が止まった理由は、イベント名でページに届きます。

  | イベント | 送るとき |
  | --- | --- |
  | `full` | 新しいルームが `--max-rooms` を超える |
  | `closed` | `--open-hours` の時間外に要求された、または動いている間に時間外になった |
  | `expired` | ルームが `--max-room-seconds` 動き続けた |
  | `failed` | bot が例外で止まった |

- ルーム ID は既定で `[a-z0-9-]` の 1〜32 文字で、bot が `serve_rooms` に別の規則を渡せます。namespace の 1 要素になります。

## オプション

| オプション | 既定値 | 内容 |
| --- | --- | --- |
| `--host` / `--port` | `127.0.0.1` / bot ごと | 待ち受けるアドレス |
| `--max-rooms` | bot ごと | 同時に動かすルームの数 |
| `--max-room-seconds` | `3600` | ルームを動かし続ける最長時間（秒） |
| `--open-hours` | なし（常時） | ルームを動かす曜日と時間。例: `'Mon-Fri 09:00-17:30 +09:00'` |
| `--allowed-origin` | なし（すべて） | ルームを要求できるページの origin。複数指定できる |

## テスト

```shell
uv run pytest
```
