# moqt-bridge-live-ingest

Live ingest bridge for publishing RTMP or SRT media into MoQT.

## Prerequisites

`--transcode` needs GStreamer; see `shared/transcode/README.md` for the packages.

## Run

Run the bridge:

```shell
make live-ingest
```

`make live-ingest` listens for RTMP on `0.0.0.0:1935`, listens for SRT on
`0.0.0.0:9000`, and publishes to the local MoQT relay. On macOS with the
Docker Compose relay running, the relay URL is resolved to the Docker Desktop
bridge host automatically.

Override the relay URL when needed:

```shell
LIVE_INGEST_MOQT_URL=https://relay.example.com:443 make live-ingest
```

Add lower renditions with `LIVE_INGEST_TRANSCODE=1 make live-ingest`. Each rendition
below the source resolution (720p / 480p / 360p) is published as `video_<height>p`
next to `video`, and the catalog lists them in one `altGroup` with `width` / `height`.

## Track Format

Video and audio objects are LOC (draft-ietf-moq-loc-01): the payload is the
codec bitstream, H.264 in Annex-B and raw AAC frames, and the capture timestamp
travels as MoQT extension header 2. The audio track's AudioSpecificConfig is
published Base64-encoded as the catalog `initData`; the video track carries its
parameter sets in band and has none.

## Publish Test RTMP

Publish a generated test video and sine audio stream with the namespace
`anon/live/test`:

```shell
make ffmpeg-rtmp
```

## Publish Test SRT

Publish an MPEG-TS test stream with the namespace `anon/live/test`:

```shell
make ffmpeg-srt
```

The bridge uses the SRT stream ID as the MoQT namespace: either the `r=` resource
of an access-control stream ID (`#!::r=anon/live/test,m=publish`) or a plain
path (`anon/live/test`). Connections without a stream ID publish under
`anon/srt/live`. Relays only let tokenless publishers into `anon/**`.

## Publish Big Buck Bunny over SRT

```shell
make ffmpeg-srt-bbb
```

The first run downloads `bbb_sunflower_1080p_30fps_normal.mp4` (263 MiB) from
download.blender.org into `assets/bbb/`, which is git-ignored; the film is then
looped into the same `anon/live/test` namespace as `make ffmpeg-srt`, re-encoded
to a 2-second GOP so the viewer's seek granularity matches the test pattern.

Big Buck Bunny is (c) 2008 Blender Foundation | www.bigbuckbunny.org, licensed
under [Creative Commons Attribution 3.0](https://creativecommons.org/licenses/by/3.0/).

## Direct CLI Options

Use `cargo run` directly when you need options that are not exposed by the Makefile helpers.

```shell
cargo run -p moqt-bridge-live-ingest -- \
  --rtmp-addr 0.0.0.0:1935 \
  --srt-addr 0.0.0.0:9000 \
  --moqt-url https://127.0.0.1:4433
```

Options:

- `--rtmp-addr`: RTMP listen address
- `--srt-addr`: SRT listen address
- `--moqt-url`: MoQT relay URL
- `--transcode`: re-encode video into the standard renditions below the source resolution
