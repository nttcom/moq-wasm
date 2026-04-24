# MoQ WASM

Rust 製の MoQT 実装と、WASM 経由で利用する browser client / examples を含むワークスペースです。

## Demo page

Being Deployed to Github Pages.

- https://nttcom.github.io/moq-wasm/

## Implementation

Supported version: draft-ietf-moq-transport-14

- Control plane
  - `CLIENT_SETUP` / `SERVER_SETUP`
  - `PUBLISH_NAMESPACE` / `NAMESPACE_OK` / `REQUEST_ERROR`
  - `SUBSCRIBE_NAMESPACE`
  - `SUBSCRIBE` / `SUBSCRIBE_OK` / `SUBSCRIBE_ERROR`
  - `UNSUBSCRIBE`
- Data plane
  - Object Datagram / Object Datagram Status
  - Subgroup Header / Subgroup Object
- Browser integration
  - `moqt-client-wasm` から draft-14 wire 実装を JS に公開
  - `js/examples/message` / `media` / `media-cmaf` / `call` / `onvif` が draft-14 フローを利用

## Modules

### moqt

- draft-14 の MoQT wire / codec / session 実装
- browser 側では `moqt-client-wasm` から wire API を共有利用

### media-streaming-format

- MSF (Media Streaming Format) catalog structures (draft-ietf-moq-msf-00)

### relay

- relay binary
- `--transport quic|webtransport` を切り替えて起動可能

### moqt-client-wasm

- browser client 用 WASM module
- draft-14 の control/data message を JS から直接利用可能
- MSF catalog JSON helper を公開

### moqt-client-onvif

- Client for IP cameras over RTSP/ONVIF (Raspberry Pi and Mac)
- Includes `moqt-onvif-client` to bridge RTSP video + ONVIF commands over MoQ

## How to run

### Local draft-14 relay for browser examples

`relay` は初回起動時に `keys/` 配下へ自己署名証明書を生成します。

```shell
make relay-browser
```

同等のコマンド:

```shell
cargo run -p relay -- --transport webtransport --port 4433
```

QUIC relay を使う場合:

```shell
make relay
```

if you want to watch tokio tasks, use tokio-console

```shell
cargo install tokio-console
tokio-console
```

#### Specify the log level

```shell
make server-trace

or

# Default setting is `DEBUG`
cargo run -p moqt-server-sample -- --log <Log Level>
```

### Run browser examples

```shell
cd js && npm install
cd js && npm run wasm
make client
```

- ブラウザ側では `https://127.0.0.1:4433` を preset から選択できます
- `make chrome` は `relay` が生成した `keys/cert.pem` を読み込み、対応する SPKI pin を付けて Chrome を起動します
- 証明書がまだ無い場合は、先に `make relay-browser` か `cargo run -p relay -- --transport webtransport --port 4433` を一度実行してください

```shell
# For Mac users
make chrome
```

### Run moqt-client-onvif

```shell
cp .env.example .env
make onvif
```

`ONVIF_IP` / `ONVIF_USERNAME` / `ONVIF_PASSWORD` are read from `.env`.

MoQ bridge:

```shell
make onvif-moq
```

`MOQT_URL` is read from `.env`.
