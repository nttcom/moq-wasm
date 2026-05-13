# relay

MoQT relay です。QUIC と WebTransport の両方を `4433` で受け付けます。

初回起動時に `relay/keys/` に自己署名証明書を生成します。

```shell
make relay
```

同等のコマンド:

```shell
cargo run -p relay
```
