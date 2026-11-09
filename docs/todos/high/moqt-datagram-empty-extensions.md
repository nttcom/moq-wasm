# [XS] Reject an object datagram with Extensions Present and an empty extension block

Next: after `ExtensionHeaders::decode` in `DatagramField::decode` (`crates/moqt/src/modules/moqt/data_plane/object/datagram_field.rs:62`), fail when the decoded block is empty.

## Goal

The receiver closes the session with PROTOCOL_VIOLATION, as draft-14 §10.3.1 requires (MUST).
Today `DatagramField::decode` accepts it as `Some` empty headers.

## Plan

The encoder has the mirror case: `Some` empty headers set `EXTENSIONS_PRESENT` (`datagram_field.rs:83`) and send a length of 0; send `None` instead. Check how the datagram receive path closes the session on a decode error.
