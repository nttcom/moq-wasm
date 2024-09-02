# MoQ WASM

Both server and browser client are written in Rust.

## Implementation

- [x] Send/Recv SETUP message
- [x] Send/Recv ANNOUNCE message
- [x] Send/Recv SUBSCRIBE message
- [x] Echo back OBJECT message
- [ ] Send/Recv GOAWAY message
- [ ] Send/Recv SUBSCRIBE_FIN/SUBSCRIBE_RST message
- [x] Transfer SUBSCRIBE message
  - [x] Manage stream of publishers
- [x] Transfer OBJECT message
  - [x] Manage subscriptions

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

### Generating public and private keys for the server

```shell
cd moqt-server-sample
mkdir keys
cd keys
openssl req -newkey rsa:2048 -nodes -keyout key.pem -x509 -out cert.pem -subj '/CN=Test Certificate' -addext "subjectAltName = DNS:localhost"

```

### Run moqt-server-sample

- `cargo run -p moqt-server-sample`

#### Specify the log level

- `cargo run -p moqt-server-sample -- -log <Log Level>`
  - Default setting is `DEBUG`

### Run moqt-client-sample

```shell
cd js
npm install
npm run dev
```

- Add a certificate and Enable WebTransport feature in Chrome

```shell
For Mac users
./scripts/start-localhost-test-chrome.sh
```
