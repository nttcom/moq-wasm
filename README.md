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

- `cd js && npm install && npm run dev`

```shell
openssl x509 -pubkey -noout -in moqt-server-sample/keys/cert.pem | openssl rsa -pubin -outform der | openssl dgst -sha256 -binary | base64
出力結果を下記コマンドに含めることで、証明書エラーを無視できる。Chromeで初回リクエストをTCPではなくするためのオプションも付ける。
Chromeを完全に終了してから下記コマンドを打たないとオプションが反映されない可能性がある
open -a "Google Chrome" --args --origin-to-force-quic-on=localhost:4433 --ignore-certificate-errors-spki-list=<ENCODE_KEY>
```
