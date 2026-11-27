# moq-chat-moderation

Chat UI backed by the Python moderation bot ([`examples/python/moq-chat-moderation`](../../../python/moq-chat-moderation/README.md)) that scores each message against configurable labels, tags matches >= 70%, and removes messages flagged as `暴言`.

## Run

1. Start a relay (`make relay`), register `chat-moderation` in [`services/bot-dispatcher`](../../../../services/bot-dispatcher/README.md) (see [`examples/python/moq-chat-moderation/README.md`](../../../python/moq-chat-moderation/README.md)), and run `make browser` + `make chrome`.
2. Open `http://localhost:5173/moq-wasm/examples/moq-chat-moderation/` in temporary-app mode (**一時トークン**).
3. Click **Join**, click **bot を参加させる**, edit labels in the sidebar if desired, and send messages. Click any message to view its per-label probability breakdown.

## Tracks

- Room namespace: `<root>/moq-chat-moderation/<room>` (override room with `?room=`).
- Page publishes messages and active label definitions to `<room>/chat` (track `chat`).
- Bot (`chat-moderation`) publishes per-message verdicts on its event timeline track.

## E2E

```shell
node tests/browser-e2e/run-chat-moderation-e2e.mjs
```
