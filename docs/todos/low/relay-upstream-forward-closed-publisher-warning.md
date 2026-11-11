# [XS] Do not warn when a Forward State update meets a closed publisher

Next: at `services/relay/src/modules/control_plane/sequences/upstream_forward.rs:37`, log a closed publisher session at DEBUG and keep WARN for the other failures.

## Goal

A publisher of an on-demand track that disconnects leaves no WARN in the relay log. Today the relay logs `failed to update the upstream Forward State error=session error: webtransport error: closed` with `forward=false` for its tracks, as the media browser E2E shows (2026-11-11, track `camera_audio`).

## Plan

Open question: whether to skip the update once the publisher's session is known to be closed, instead of sending it and reading the error.
