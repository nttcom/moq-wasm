# [M] Send and check the PATH and AUTHORITY of a native QUIC session as draft-14 requires

Next: fill `path` and `authority` from the `moqt://` URI in `send_client_setup` (`crates/moqt/src/modules/moqt/version/draft14/setup.rs:34`, `:41`, both `None`), then check them in `receive_client_setup` (`:78`) and `receive_server_setup` (`:54`).

## Goal

A client connecting to a `moqt://` URI sends its authority and path (draft-14 §9.3.2.1, §9.3.2.2, MUST). One from a server or over WebTransport closes the session with `InvalidPath` / `InvalidAuthority`; one that is not a URI part, with `MalformedPath` / `MalformedAuthority`.

## Plan

A server serves every authority and path: no relay here routes by host or path (the token decides app and namespace), and behind the UDP passthrough load balancer the relay receives the public name, not its own address. Add a list of served names only when one endpoint must tell domains apart. draft-18 was not checked.
