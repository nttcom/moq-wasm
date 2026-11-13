# [XS] Reject a draft-14 control message whose fields end before its Message Length

Next: in `decode_payload` (`crates/moqt/src/modules/moqt/data_plane/codec/control_message_decoder.rs:196`), return a fatal error when the cursor has bytes left, as draft-18 does (`version/draft18/messages.rs:288`).

## Goal

draft-14 §9: "If the length does not match the length of the Message Payload, the receiver MUST close the session with PROTOCOL_VIOLATION."
Reproduced: an UNSUBSCRIBE with 4 bytes after its Request ID decodes as UNSUBSCRIBE.
