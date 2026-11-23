# [M] Withdraw only the publication a PUBLISH_NAMESPACE_DONE ends

Next: key a client's namespace publication by its request, not by `(session, namespace)`, in `table.unregister_publish_namespace` (`services/relay/src/modules/control_plane/sequences/publish_namespace_done.rs:35`) and `register_publish_namespace` (`publish_namespace.rs:59`).

## Goal

A client that withdraws a namespace and publishes it again at once stays published, and its namespace subscribers see it published.
Reproduced by `sdk/python/tests/test_media.py`'s `publish_again_and_again` tests (about 1 run in 25): in draft-18 the DONE ends the old request's stream and the new PUBLISH_NAMESPACE opens another, so the relay can read the new one first. The relay log then shows `PUBLISH_NAMESPACE received` (request 2), `PUBLISH_NAMESPACE_OK sent`, then `PUBLISH_NAMESPACE_DONE received` for request 0. The late DONE removes the namespace and sends NAMESPACE_DONE, and the watcher never hears of the publication that is still live.

## Plan

Draft-14 sends both on the control stream, so this happens only in draft-18.
Do not paper over it in the tests: a test that waits between the withdrawal and the next publication hides the relay bug.
