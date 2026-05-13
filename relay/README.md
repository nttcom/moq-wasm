# relay

MoQT relay. It accepts both QUIC and WebTransport on port `4433`.

On the first run, it generates a self-signed certificate under `relay/keys/`.

```shell
make relay
```

Equivalent command:

```shell
cargo run -p relay
```
