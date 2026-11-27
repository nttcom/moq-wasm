# embed

Minimal playback demo using the `<moq-player>` custom element (`moq-wasm/element`).

## Run

```shell
make relay
make browser
make chrome
```

Publish any MSF broadcast to the relay (for example via [`media/publisher`](../media/README.md) or `make ingest-gateway` + `make ffmpeg-srt`), then open `http://localhost:5173/moq-wasm/examples/embed/`, enter the relay URL and Track Namespace, and click **再生**.

## Usage

The page renders `<moq-player>` and prints the corresponding HTML snippet:

```html
<script type="module">
  import 'moq-wasm/element'
</script>
<moq-player src="https://127.0.0.1:4433" namespace="anon/live/test" controls></moq-player>
```

Set `player.token` before connecting when subscribing outside `anon/**`.
