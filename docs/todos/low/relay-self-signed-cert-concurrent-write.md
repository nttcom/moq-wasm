# [S] Keep relays that start together from writing mismatched test certificates

Next: make `create_certs_for_test_if_needed` (`services/relay/src/main.rs:32`) write `cert.pem` and `key.pem` so that a concurrent relay reads a matching pair, e.g. write both to a temp dir and rename it into place.

## Goal

The Python SDK suite passes on its first parallel run in a fresh worktree. Today several relays started by pytest-xdist see no `services/relay/keys/`, each writes its own pair (`write_self_signed_cert`, `crates/moqt/src/modules/moqt/domains/self_signed_cert.rs:14`), and a relay loads one process's cert with another's key: `KEY_VALUES_MISMATCH`, 92 relay tests errored.
Deleting the keys and starting one relay first made it pass. Not checked whether CI hits it.
