# [XS] Treat a PUBLISH_DONE whose request the subscriber already withdrew as routine

Next: add the draft-18 `TransportSendError::ClosedStream` case to `subscriber_left_reason` (`services/relay/src/modules/data_plane/egress/group_sender.rs:1053`), or make `send_publish_done` check it.

## Goal

The media E2E logged `ERROR failed to send PUBLISH_DONE error=closed stream` once in four runs. A browser unsubscribed from `camera` just as the publisher stopped it. On draft-18 the relay had already taken the request stream (`ControlChannel::notify`), so it returned `ClosedStream`. This is the same routine race as `peer_stopped`, and it should log at DEBUG.
