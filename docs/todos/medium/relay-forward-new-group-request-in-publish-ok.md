# [S] Pass a downstream PUBLISH_OK's NEW_GROUP_REQUEST upstream

Next: in `services/relay/src/modules/control_plane/sequences/downstream_publish.rs:98`, hand the PUBLISH_OK's `new_group_request` (`moqt::PublisherInitiatedSubscription`) to `sequences/new_group_request.rs`, as a SUBSCRIBE's is.

## Goal

A subscriber that answers the relay's PUBLISH with NEW_GROUP_REQUEST gets a new group: draft-18 §10.2.13 says a relay that receives one for an Established subscription MUST send a REQUEST_UPDATE upstream, gated like the others.
Read from the code, not run: `downstream_publish` reads only Forward, priority, filter and delivery timeout from the PUBLISH_OK.
