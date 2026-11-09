# [XS] Say the relay also accepts WebSocket on its port

Next: edit `services/relay/README.md:3` ("accepts QUIC and WebTransport on one port") and the `RELAY_PORT` row at `services/relay/README.md:24`.

## Goal

The relay README names every transport the relay serves. `services/relay/src/main.rs:54` spawns `moqt::NATIVE` on `config.port`, which also accepts QMux over WebSocket (`wss://`) on the TCP port of that number (`docs/architecture/moqt/architecture.md`, `NATIVE` row).
