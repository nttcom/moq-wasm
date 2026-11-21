# [S] Let a subscription read by object report the end of a group

Next: add a way for `client::Subscription::next` (`crates/moqt/src/modules/client/subscription.rs`) to return End of Group status objects, which it skips today.

## Goal

The Python SDK's `decode_pictures` flushes a group's decoder when its group ends, so the last B-frame pictures of a group read through `RemoteBroadcast.follow()` come out at once instead of when a later group evicts the decoder.
