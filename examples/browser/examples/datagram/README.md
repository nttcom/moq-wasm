# datagram

Side-by-side comparison of reliable MoQT Subgroup streams vs unreliable Datagrams at 60 Hz using a live cursor pad and latency chart.

## Run

Run each command in a separate terminal:

```shell
make relay
make browser
make chrome
```

Open `http://localhost:5173/moq-wasm/examples/datagram/`, pick a relay, and click **開始**. Move the cursor over either pad (or leave **自動で動かす** checked) to compare both delivery modes.

To see head-of-line blocking vs packet drops, inject packet loss with macOS Network Link Conditioner or Linux `tc netem`:

```shell
sudo tc qdisc add dev <dev> root netem loss 5% delay 30ms
```

## Tracks

The page publishes and subscribes to two tracks under `<root>/datagram/<id>` on the same session (1 group per second, 60 objects/group, `retentionMs: 0`):

| Track      | Delivery                    | Behavior under packet loss                     |
| ---------- | --------------------------- | ---------------------------------------------- |
| `stream`   | Subgroup stream             | Retransmitted in order; stalls until recovered |
| `datagram` | Datagram (`datagram: true`) | Unreliable, unordered; drops without stalling  |

Each lane reports received count, lost count, reordered count, latest/median/p95 latency, and 10 s max gap.

## E2E

```shell
node tests/browser-e2e/run-datagram-e2e.mjs
```
