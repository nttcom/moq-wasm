# [M] Keep whichever of a publisher's PUBLISH and the relay's SUBSCRIBE completes first

Next: in `register_published_subscription` (`services/relay/src/modules/control_plane/sequences.rs:199`), compare when the relay's SUBSCRIBE_OK arrived with when it sends PUBLISH_OK.

## Goal

A track the relay both SUBSCRIBEs to and receives a PUBLISH for from one session keeps the subscription that completed first and ends the other, so no group is cut by switching. Model: `origin_sessions_current` in `docs/architecture/moqt/models/origin_sessions.qnt`.

## Plan

- Only the relay decides; `Origin` follows the UNSUBSCRIBE or PUBLISH_ERROR (an `Origin` that ends one side itself breaks the relay's track, as `robot_teleop_e2e` showed).
- Check first: today PUBLISH always replaces the relay's SUBSCRIBE, and `Origin` starts a PUBLISH at the next group, so the rest of the current group may never reach subscribers.
- Check: `add_upstream_subscription_to_track` returns `NotTaken` for a SUBSCRIBE whose publisher already PUBLISHed; whether that SUBSCRIBE is unsubscribed, or keeps an outlet sending.
