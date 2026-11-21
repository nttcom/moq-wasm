# [S] Start a PUBLISH's objects where its PUBLISH_OK's filter says

Next: replace `FilterType::NextGroupStart` in `Origin::publish` (`crates/moqt/src/modules/moqt/domains/origin.rs:433`) with the filter of the accepted subscription.

## Goal

A PUBLISH answered with Largest Object gets the rest of the current group, as a SUBSCRIBE with that filter does (draft-14 §9.14).
Read from the code, not run: today the objects start at the next group whatever the filter says.

## Plan

The relay answers a PUBLISH with a filter of its own (`publish_accept_options`, `services/relay/src/modules/session/subscription.rs:54`), so check what the relay then receives before changing it.
