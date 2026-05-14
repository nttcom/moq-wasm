# Cross-Protocol Test

This application is for manually verifying video Pub/Sub across QUIC and WebTransport.

## Architecture

```
                              [Relay]
                          (dual protocol)
[QUIC Publisher]           port 4433               [Browser Subscriber]
 Rust native               (QUIC + WT)             moqtail (WASM) connection
 Camera video                    ↓                 fMP4 playback with MSE
   ↓                       Relays data across
 ffmpeg (child process)    both protocols           [QUIC Subscriber]
   ↓                                                 Rust native
 fMP4 -> MoQT publishing                             Writes fMP4 to stdout
```

This verifies both QUIC -> QUIC and QUIC -> WebTransport.

## Data Format

- Container: fMP4 (fragmented MP4)
- Video codec: H.264 (yuv420p, MSE compatible)
- Audio: none (video only)

### Mapping to the MoQT Data Model

> **Note**: This implementation does not conform to the MoQT catalog specification
> (draft-ietf-moq-catalogformat). In standard CMAF over MoQT, the init segment is
> delivered through the catalog `initData` field. This implementation does not use
> catalogs and instead maps the init segment to Object 0 in each Group. It is a
> simplified implementation for validating cross-protocol relay behavior.

```
Track: video
  Group N (= 1 GoP):
    Subgroup 0:
      Object 0: init segment (ftyp+moov)
      Object 1: media segment (moof+mdat)
```

- 1 Group = 1 Subgroup = 1 GoP (1-second interval)
- Object 0 sends the init segment (ftyp+moov) every time to support late joiners
- Object 1 sends the media segment (moof+mdat)

## Prerequisites

- macOS (camera capture via avfoundation)
- ffmpeg / ffplay installed
- Node.js (for the browser subscriber)
- Relay server available in dual-protocol mode (QUIC + WebTransport)

## Usage

### 1. Start the Relay

```sh
cargo run -p relay
```

### 2. Start the Publisher

```sh
cargo run -p quic-publisher
```

The log prints `using namespace namespace="live-XXXX"`.
The publisher waits for a subscriber, then starts ffmpeg and begins sending video.

### 3a. Browser Subscriber (Recommended)

```sh
cd examples/cross-protocol-test/browser-subscriber
npm install
npm run dev
```

Open http://localhost:5173/ in a browser, enter the namespace, and press the Subscribe button.

### 3b. QUIC Subscriber

Pipe to ffplay for real-time playback:

```sh
cargo run -p quic-subscriber -- <namespace> video 2>/dev/null | ffplay -
```

## QUIC Publisher

The publisher starts ffmpeg as a child process and delegates camera capture, encoding, and fMP4 muxing to it.
The Rust side reads fMP4 bytes from ffmpeg stdout, parses them box by box, and sends them over MoQT.

ffmpeg is not started until a subscriber connects. This prevents unnecessary buffering.

### ffmpeg Command

```sh
ffmpeg \
  -f avfoundation -framerate 30 -video_size 640x480 -i "0:none" \
  -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 30 \
  -f mp4 -movflags frag_keyframe+empty_moov+default_base_moof \
  pipe:1
```

- `-pix_fmt yuv420p`: pixel format supported by MSE
- `-g 30`: keyframe every second at 30 fps
- `default_base_moof`: movie-fragment-relative addressing required by MSE

### Processing Flow

1. Connect to the Relay over QUIC and publish the namespace
2. Wait for a subscriber connection
3. Start ffmpeg as a child process when a subscriber connects
4. Read fMP4 boxes from stdout (first 8 bytes: 4-byte size + 4-byte type)
5. Keep `ftyp` + `moov` boxes as the init segment
6. Pack each `moof` + `mdat` pair into a MoQT Object as a media segment and send it

## Browser Subscriber

Browser application using moqtail (WASM). It connects to the Relay with WebTransport and plays received fMP4 in real time using MSE (Media Source Extensions).

### Processing Flow

1. Connect to the Relay (port 4433) with WebTransport via moqtail
2. Establish the MoQT session (SETUP)
3. Subscribe using the namespace and track name from the UI
4. Parse avcC from the init segment and automatically detect the codec string
5. Initialize MSE MediaSource and SourceBuffer
6. Extract data from received Objects:
   - Object 0 (init segment): append to SourceBuffer only once
   - Object 1 (media segment): append to SourceBuffer
7. Seek automatically to the live edge when more than 2 seconds of data is buffered

### Technical Notes

- The relay self-signed certificate hash is injected at Vite build time and passed through WebTransport `serverCertificateHashes`
- Restart Vite if the relay certificate is regenerated

## QUIC Subscriber

Rust native CLI. It connects to the Relay over QUIC and writes received fMP4 to stdout.
It can be piped to ffplay for real-time playback or saved to a file for inspection.

### Processing Flow

1. Connect to the Relay (port 4433) over QUIC
2. Establish the MoQT session (SETUP)
3. Subscribe using the namespace and track name from command-line arguments
4. Extract data from received Objects:
   - Object 0 (init segment): write to stdout only once; skip later occurrences
   - Object 1 (media segment): write directly to stdout

### Design Notes

- tracing output goes to stderr; stdout is reserved for fMP4 data
- The init segment is written only for the first group. Writing it every time breaks the fMP4 stream with a duplicated MOOV atom
- Certificate verification is skipped (`verify_certificate: false`)
- Pipe to `ffplay -` for real-time playback
