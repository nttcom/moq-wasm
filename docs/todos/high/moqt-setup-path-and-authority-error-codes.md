# [M] Close a session with the PATH and AUTHORITY error codes draft-14 requires

Next: decide what a server does with a client's PATH and AUTHORITY; `receive_client_setup` (`crates/moqt/src/modules/moqt/version/draft14/setup.rs:78`) returns them unchecked and no code reads them.

## Goal

A PATH or AUTHORITY from a server, over WebTransport or that the server does not serve closes the session with `InvalidPath` / `InvalidAuthority`; one that is not a URI part, with `MalformedPath` / `MalformedAuthority` (draft-14 §9.3.2.1, §9.3.2.2, MUST).
Read from the code, not run: nothing sends these four codes, and a PATH or AUTHORITY that is not UTF-8 fails the handshake without one.

## Plan

Open: which paths and authorities a server serves is the application's to say. draft-18 was not checked.
