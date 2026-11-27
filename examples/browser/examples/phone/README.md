# phone

Browser telephone UI for placing and answering SIP calls through [`services/sip-gateway`](../../../../services/sip-gateway/README.md) using `PhoneCall` (`moq-wasm/media`).

## Run

Requires `services/sip-gateway`, a MoQT relay, and an app server (default `http://127.0.0.1:8097`, override with `?appServerUrl=`) that provides `/token`, `/calls`, `/incoming`, `/answer`, and `/decline`.

Run the full local stack (Asterisk test trunk, VTS, relay, `sip-gateway`, dev app server, and Playwright verification):

```shell
./tests/sip-gateway-e2e/run.sh
```

To use the page interactively against a running relay and app server:

```shell
make browser
MOQT_FAKE_MEDIA=0 make chrome
```

Open `http://localhost:5173/moq-wasm/examples/phone/`, pick the relay, and click **接続**.

## Features

- **Outbound calls**: Enter an E.164 phone number (e.g. `+819012345678`) and click **発信**.
- **Inbound calls**: Long-polls `/incoming` and displays caller ID with **応答** / **拒否** buttons.
- **In-call controls**: Live call state (`dialing` / `ringing` / `answered` / `ended`), audio bitrate readout, RFC 4733 DTMF keypad (`0`–`9`, `*`, `#`), and **切る** (hang up).
