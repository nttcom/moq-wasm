# MoQT echo server (FastAPI)

A FastAPI process that also listens for MoQT sessions. Every object received
on a published track is written back to all sessions subscribed to the same
track name, so a publisher and a subscriber connected to this process see
each other's data without a relay. `GET /` reports session and track counters.

## Run

```bash
cd examples/python/echo-server
uv venv && source .venv/bin/activate
uv pip install maturin && (cd ../../../bindings/python && maturin develop)
uv pip install fastapi uvicorn
openssl req -x509 -nodes -days 1 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
  -keyout key.pem -out cert.pem -subj /CN=localhost \
  -addext "subjectAltName=DNS:localhost,IP:127.0.0.1"
uvicorn echo_server:app --port 8000
```

Environment: `MOQT_PORT` (default 4433), `MOQT_CERT`, `MOQT_KEY`.

## Try it with moq-cli

Publisher (H.264 test pattern, PUBLISH over raw QUIC):

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x360:rate=30 -c:v libx264 -preset ultrafast \
  -tune zerolatency -g 30 -f h264 - \
  | cargo run -p moq-cli -- publish --relay moqt://127.0.0.1:4433 --track demo/video --insecure
```

Subscriber (receives the echoed track):

```bash
cargo run -p moq-cli -- subscribe --relay moqt://127.0.0.1:4433 --track demo/video --insecure \
  | ffplay -f h264 -
```

```bash
curl -s localhost:8000/
```

Group boundaries are preserved on the echoed track, but group ids are
reassigned by the server's `TrackWriter`; objects that arrive while a track
has no subscriber are dropped.
