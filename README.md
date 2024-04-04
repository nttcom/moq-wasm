# loc-over-moqt

~~"Low Overhead Media Container" (LOC) over~~ "Media over QUIC Transport" (MOQT)

Both server and browser client are written in Rust.

## Implementation

- [x] Send/Recv SETUP message
- [x] Send/Recv ANNOUNCE message
- [x] Send/Recv SUBSCRIBE message
- [x] Echo back OBJECT message
- [ ] Send/Recv GOAWAY message
- [ ] Send/Recv SUBSCRIBE_FIN/SUBSCRIBE_RST message
- [ ] Transfer SUBSCRIBE message
  - [ ] Manage stream of publishers
- [ ] Transfer OBJECT message
  - [ ] Manage subscriptions

## Modules

### moqt-core

- Core module for both server and client
- Includes handlers and data structures

### moqt-server

- Module for server application
  - Only for WebTransport
    - Using [`wtransport`](https://github.com/BiagioFesta/wtransport)

### moqt-server-sample

- Sample server application

### moqt-client-sample

- Module for browser client and sample browser client application

## How to run

### サーバ用の公開鍵と秘密鍵を作成する

```shell
cd moqt-server-sample
mkdir keys
cd keys
openssl req -newkey rsa:2048 -nodes -keyout key.pem -x509 -out cert.pem -subj '/CN=Test Certificate' -addext "subjectAltName = DNS:localhost"

```

### moqt-server-sample の実行

- `cargo run -p moqt-server-sample`

### moqt-client-sample の実行

```shell
cd js
npm install
npm run dev
```

- chrome の WebTransport 機能の ON と証明書の追加

```shell
Mac の場合
./scripts/start-localhost-test-chrome.sh
```
