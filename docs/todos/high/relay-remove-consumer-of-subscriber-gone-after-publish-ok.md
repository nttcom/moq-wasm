# [S] Remove the consumer of a subscriber that left right after its PUBLISH_OK

Next: in `services/relay/src/modules/control_plane/sequences/downstream_publish.rs:148`, unregister the downstream subscription registered at `:103` when `start_egress` reports the subscriber gone, as `end_if_publisher_gone` does for a producer (`subscribe/pending_producer_task.rs:265`).

## Goal

No consumer of a closed session stays on a track.
Reproduced: the subscriber's cleanup finishes between its PUBLISH_OK and `register_downstream_subscription`; the task bails and the consumer stays.
The track then keeps its upstream subscription after the last real subscriber left, so an on-demand publisher keeps sending at Forward 1 until every producer of the track is gone.
