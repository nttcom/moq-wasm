# moqt-bridge-live-ingest

Publishes RTMP or SRT input into a MoQT relay as the tracks described in
[`crates/publisher`](../publisher/README.md).

## Run

```shell
make relay
make live-ingest
```

| Input | Listens on | Namespace |
| --- | --- | --- |
| RTMP | `0.0.0.0:1935` | Application name of the URL: `rtmp://host:1935/anon/live/test/<key>` |
| SRT | `0.0.0.0:9000` | Stream id: `anon/live/test` or `#!::r=anon/live/test,m=publish`; `anon/srt/live` when absent |

Tokenless publishers may only use `anon/**`.

| Option | Effect |
| --- | --- |
| `LIVE_INGEST_MOQT_URL=https://relay.example.com:443 make live-ingest` | Publish to another relay (default: the local one) |
| `make live-ingest-transcode` | Add `video_720p` / `video_480p` / `video_360p` renditions. Needs GStreamer, see [`crates/transcode`](../transcode/README.md) |
| `make live-ingest-stats` | Redraw the QUIC statistics of every relay connection on stdout once a second |

## Send a test stream

| Command | Source |
| --- | --- |
| `make ffmpeg-rtmp` | Test pattern and 1 kHz tone over RTMP |
| `make ffmpeg-srt` | Test pattern and 1 kHz tone over SRT |
| `make ffmpeg-srt-bbb-local` | Big Buck Bunny, looped, to `localhost:9000` |
| `make ffmpeg-srt-bbb-remote` | Big Buck Bunny to `relay-1.moqt.research.skyway.io:9000`; no local bridge or relay needed |

All of them publish `anon/live/test`. Watch it with the
[Live Viewer](../../examples/browser/examples/live-viewer/README.md): relay
`https://127.0.0.1:4433` for the local commands,
`https://relay-1.moqt.research.skyway.io:443` for the remote one.

The Big Buck Bunny commands download the film (263 MiB) into the git-ignored
`assets/bbb/` on first use and re-encode it with a 2-second GOP.
Big Buck Bunny is (c) 2008 Blender Foundation | www.bigbuckbunny.org,
[CC BY 3.0](https://creativecommons.org/licenses/by/3.0/).

## Packet loss

SRT is read with a 4 MiB UDP receive buffer. When MPEG-TS packets are lost
anyway, the frame they cut and the frames predicted from it are dropped until
the next keyframe; each loss is logged as a warning and the SRT statistics are
logged when a stream ends. Why:
[architecture](../../docs/architecture/moqt-bridge-live-ingest/architecture.md).

## Delivery log

```shell
RUST_LOG=info,publisher::delivery=debug make live-ingest
```

Writes one line per sample as it enters the publisher (`stage="ingest"`) and
as it is sent to the relay (`stage="publish"`, with namespace, group id and
LOC capture timestamp). The Live Viewer's delivery check reads this log; see
its README.

## CLI

```shell
cargo run -p moqt-bridge-live-ingest -- \
  --rtmp-addr 0.0.0.0:1935 \
  --srt-addr 0.0.0.0:9000 \
  --moqt-url https://127.0.0.1:4433 \
  [--transcode] [--stats]
```
