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

## OpenTelemetry

The relay initializes OTLP trace and log exporters from OpenTelemetry environment
variables.

Example OTLP/HTTP protobuf configuration for a SaaS backend such as Honeycomb:

```shell
OTEL_SERVICE_NAME=moqt-relay
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_ENDPOINT=https://api.honeycomb.io
OTEL_EXPORTER_OTLP_HEADERS=x-honeycomb-team=your-api-key,x-honeycomb-dataset=moqt-relay
```

`OTEL_EXPORTER_OTLP_HEADERS` is optional. Set it when your OTLP backend requires
request metadata such as an API key or authorization token.
