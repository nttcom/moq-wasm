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

Add lower renditions with `make live-ingest-transcode` (or
`LIVE_INGEST_TRANSCODE=1 make live-ingest`). Each rendition
below the source resolution (720p / 480p / 360p) is published as `video_<height>p`
next to `video`, and the catalog lists them in one `altGroup` with `width` / `height`.

## Transport stream loss

The SRT listener reads with a 4 MiB UDP receive buffer: a keyframe arrives as
a burst of several hundred kilobytes within a few milliseconds, and the 64 KiB
that srt-tokio uses by default overflowed whenever the reader was not scheduled
at once, with the lost datagrams rarely recovered before their delivery time.
The MPEG-TS demuxer checks continuity counters; when packets are still lost,
the frame they cut is dropped along with the frames predicted from it until
the next keyframe, instead of being published corrupt for every viewer's
decoder to fail on. Each loss is logged as a warning, and the SRT statistics
are logged when a stream ends.

## Track Format

Video and audio objects are LOC (draft-ietf-moq-loc-01): the payload is the
codec bitstream, H.264 in Annex-B and raw AAC frames, and the capture timestamp
travels as MoQT extension header 2. The audio track's AudioSpecificConfig is
published Base64-encoded as the catalog `initData`; the video track carries its
parameter sets in band and has none.

## CMAF Tracks

Every media track has a CMAF sibling named with a `_cmaf` suffix (`video_cmaf`,
`video_480p_cmaf`, `audio_cmaf`) declared with `packaging: cmaf`
(draft-ietf-moq-cmsf-01). Its init segment travels Base64-encoded in the catalog
`initData` (§3.1) and every object is one `moof` + `mdat` fragment holding one
sample (§3.3). Groups start on keyframes and take the same ids as the LOC track
for the same presentation time, so the LOC and CMAF versions of a rendition are
interchangeable; each format forms its own switching set (`altGroup` 1 for LOC,
2 for CMAF).

## FETCH

Every object is numbered and kept for 30 seconds from the moment the bridge
produces it, whether or not anything is subscribed, and a standalone FETCH for
a cached range is answered from that cache (draft-ietf-moq-transport-14
§9.16). The relay forwards a FETCH upstream when its own cache cannot cover the
range, so a viewer can rewind into the part of the stream that predates the
relay's first subscriber. A subscriber that joins while a group is open starts
receiving at the next group so the live and cached object ids agree.

## Group Alignment

The source video track and its transcoded renditions form a CMSF switching set
(draft-ietf-moq-cmsf-01 §3.2): the transcoder is asked for a keyframe at every
source keyframe and each rendition group takes the group id the source assigned
to that presentation time, so the same group id names the same instant on every
track. A rendition that misses a source keyframe keeps writing into its current
group and announces the skipped ids with the Prior Group ID Gap header when it
catches up. Re-subscribing to a track continues its group numbering rather than
restarting it.

The audio tracks follow the same boundaries: an audio sample belongs to the
group of the latest video keyframe at or before it, so audio groups start at
the same keyframes with the same ids as the video groups. Audio before the
first keyframe is dropped as the video before it is. A viewer therefore replays
the audio of a video group range by fetching the same range on the audio
track, and a subscriber joining late starts both tracks at the same keyframe.

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
