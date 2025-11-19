# MoQ WASM

Both server and browser client are written in Rust.

## Demo page

Being Deployed to Github Pages.

- https://nttcom.github.io/moq-wasm/

## Implementation

Supported version: draft-ietf-moq-transport-10

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
  - [ ] FETCH
  - [ ] FETCH_OK
  - [ ] FETCH_ERROR
  - [ ] FETCH_CANCEL
- [x] Data Streams
  - [x] Datagram
  - [x] Subgroup Stream
- [ ] Features
  - [x] Manage Publisher / Subscriber
  - [x] Forword Messages
  - [ ] Priorities
  - [x] Object Cache

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

- `make server`

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

### Run moqt-client-sample

```shell
cd js && npm install
make client
```

- Add a certificate and Enable WebTransport feature in Chrome

```shell
# For Mac users
make chrome
```
