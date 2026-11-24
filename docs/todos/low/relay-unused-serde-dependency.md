# [XS] Drop the relay's unused `serde` and `serde_json` dependencies

Next: remove `serde` (`services/relay/Cargo.toml:20`) and the dev-dependency `serde_json` (`:26`), then run `cargo test -p relay`.

## Goal

`services/relay` builds without them: no file under `services/relay/` names `serde::`, `Serialize`, `Deserialize` or `serde_json::`.
The VTS request and response types they served moved to `auth_token::VtsClient`.
