# MoQ WASM

Both server and browser client are written in Rust.

## Implementation

Supported version: draft-ietf-moq-transport-06

- [ ] Control Messages
  - [x] CLIENT_SETUP / SERVER_SETUP
  - [ ] GOAWAY
  - [x] ANNOUNCE
  - [x] SUBSCRIBE
  - [ ] SUBSCRIBE_UPDATE
  - [ ] UNSUBSCRIBE
  - [x] ANNOUNCE_OK
  - [x] ANNOUNCE_ERROR
  - [ ] ANNOUNCE_CANCEL
  - [ ] TRACK_STATUS_REQUEST
  - [x] SUBSCRIBE_ANNOUNCES
  - [ ] UNSUBSCRIBE_ANNOUNCES
  - [x] SUBSCRIBE_OK
  - [x] SUBSCRIBE_ERROR
  - [ ] SUBSCRIBE_DONE
  - [ ] MAX_SUBSCRIBE_ID
  - [x] ANNOUNCE
  - [ ] UNANNOUNCE
  - [ ] TRACK_STATUS
  - [x] SUBSCRIBE_ANNOUNCES_OK
  - [x] SUBSCRIBE_ANNOUNCES_ERROR
- [x] Data Streams
  - [x] Datagram
  - [x] Track Stream
  - [x] Subgroup Stream
- [ ] Features
  - [x] Manage Publisher / Subscriber
  - [x] Forword Messages
  - [ ] Priorities
  - [ ] Object Cache

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
  - Supported Roles: PubSub

### moqt-client-sample

- Module for browser client and sample browser client application
  - Supported Roles: Publisher, Subscriber, PubSub

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

- `cargo run -p moqt-server-sample -- --log <Log Level>`
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
