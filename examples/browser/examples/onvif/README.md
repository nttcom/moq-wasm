# onvif

Remote ONVIF camera viewer and PTZ controller working with [`services/ingest-gateway`](../../../../services/ingest-gateway/README.md#pull).

## Run

Configure `ONVIF_IP`, `ONVIF_USERNAME`, and `ONVIF_PASSWORD` in `.env`, then run each command in a separate terminal:

```shell
make relay
make onvif
make browser
make chrome
```

Open `http://localhost:5173/moq-wasm/examples/onvif/`, pick the local relay, and click **Connect**.

## Controls & Tracks

- **Controller tab**: Press and hold the directional pad to send `continuous` pan/tilt commands (releasing sends `stop`), or click the center button to recenter.
- **Commands tab**: Send explicit JSON commands (`absolute`, `relative`, `continuous`, `stop`, `center`) with custom `pan`, `tilt`, `zoom`, and `speed` values.
- **Advanced Settings** (gear icon): Configure namespaces and track names:

| Setting             | Default             | Direction                     |
| ------------------- | ------------------- | ----------------------------- |
| Subscribe Namespace | `anon/onvif/client` | Camera video → browser        |
| Publish Namespace   | `anon/onvif/viewer` | Browser PTZ commands → camera |
| Command Track       | `command`           | JSON command objects          |
