# `obs-moqt` Architecture

## Status
Living document. Update alongside any change to the libobs boundary, the stream lifecycle, or the service settings.

## Scope
`crates/obs-moqt` is an OBS Studio plugin (`cdylib`) that hands the packets of OBS's own H.264 and AAC encoders to `crates/broadcast` (track layout in [`broadcast/architecture.md`](../broadcast/architecture.md)). OBS keeps the capture, encoding, **Start Streaming** button, status bar, and reconnect policy; the plugin adds only a destination.

```mermaid
flowchart LR
    Frontend["OBS frontend (Start Streaming)"] --> Service["moqt_service (settings, protocol MoQT)"]
    Frontend --> Output["moqt_output"]
    Encoders["OBS encoders (H.264, AAC)"] -- "encoded_packet" --> Output
    Output --> Input["MediaInput"]
    Input -- "MediaEvent" --> Writer["broadcast::BroadcastWriter"]
    Writer --> Relay["MoQT Relay"]
```

## Modules

| Module | Role |
| --- | --- |
| `libobs` | Hand-written declarations of the libobs C ABI in use (structs, constants, functions) |
| `service` | Registers `moqt_service`: stateless, names the `MoQT` protocol, defaults the keyframe interval |
| `output` | Registers `moqt_output`: stream lifecycle, relay session, statistics |
| `media_input` | Encoder packets → `mediapack::MediaEvent` (parameter sets, one clock) |
| `publish_settings` | Service settings → `broadcast::MoqtTarget` and namespace |

`media_input` and `publish_settings` hold no libobs calls and are the only modules built for unit tests, because libobs symbols exist only inside OBS.

## libobs Boundary
- **No build-time dependency on OBS**: `libobs.rs` declares only the structs and functions the plugin uses, following the OBS Studio 32.0 headers; `build.rs` lets the macOS linker leave them undefined until OBS loads the plugin. `obs_module_ver` reports 32.0, the oldest libobs that loads it (tested with 32.0.2).
- **Struct sizes**: `obs_register_output_s` and `obs_register_service_s` take the struct size, so a newer libobs with longer structs still accepts these.
- **Panics**: A panic leaving an `extern "C"` callback aborts OBS and any recording in progress, so `start`, `stop`, `encoded_packet`, and `destroy` catch it and report failure instead.

## Service Selection
The frontend picks the output by protocol: `moqt_service` returns `MoQT`, and `moqt_output` is the only output registered for it. The output reads `server`, `namespace`, `bearer_token`, `bearer_token_file`, and `insecure_skip_tls_verify` from the service's settings at each start, so the service holds no state. The URL, the namespace, and exactly one of the token and the token file are required: the plugin has no tokenless (`anon/**`) mode.

OBS's **Settings → Stream** page lists only its built-in services, so the service is set through the profile's `service.json` or obs-websocket's `SetStreamServiceSettings` (see the crate README). A settings dock or the frontend's own service list would replace that step without changing the output.

## Stream Lifecycle
One tokio runtime per output instance runs the relay session.

| OBS call | Plugin behavior |
| --- | --- |
| `start` (frontend thread) | Reads the settings and initializes the encoders, then returns; a task connects and announces the namespace, and only then begins the data capture. A failed connect ends with `OBS_OUTPUT_CONNECT_FAILED` and the error text |
| `encoded_packet` (encoder threads) | Converts and queues the packet without waiting for the relay. An error ends the session with `OBS_OUTPUT_DISCONNECTED`, which OBS answers with its reconnect settings by calling `start` again |
| `stop` | Joins the connect task, ends the data capture, and closes the session (waiting at most 2 s for the relay) |

- **Stop during connect**: The connect task has no await point between storing the session and beginning the data capture, so after `stop` aborts and joins it, either both happened or neither.
- **Token expiry**: The relay closes the session at the token's `exp`. A `bearer_token` is sent once, in `CLIENT_SETUP`, so OBS's reconnects fail until the settings hold a new one. A `bearer_token_file` is read at each `start` (a missing or empty file fails the start) and, while the session is up, by a `broadcast::TokenRefreshTask` that refreshes the session's token with the file's content whenever it changes.
- **Relay loss**: `BroadcastWriter::push` fails only once the publish task stops, which follows the transport's idle timeout (about 30 s on QUIC); until then packets queue as for any `broadcast` publisher.

## Media Conversion
- **Parameter sets**: x264 in OBS puts SPS/PPS only in the encoder's extra data, so the first video packet seeds `mediapack`'s `ParameterSetTracker` with it and every keyframe is completed in band.
- **Clock**: Packet timestamps are converted from the encoder timebase to microseconds and shifted so the first video packet's DTS is zero (B-frame reordering makes it negative). Audio uses the same shift; audio before the first video packet is left out.
- **Keyframe interval**: `moqt_service` sets `keyint_sec` to 2 when the encoder settings leave it at 0, because OBS's simple output mode has no such setting and x264's default is 250 frames.
- **Codecs**: `moqt_output` declares `h264` and `aac`.

## Testing
- Unit and property tests cover `media_input` and `publish_settings` ([proptest.md](proptest.md)).
- The libobs callbacks are checked by running OBS itself: an isolated OBS (`CFFIXED_USER_HOME` on macOS) with a prepared profile, `--startstreaming`, obs-websocket for stop and start, and `moq-cli subscribe` decoding the received video.
