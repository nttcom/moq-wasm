# [S] Let `subscribe_namespace` send a draft-18 prefix of 0 fields

Next: decide what string means "every namespace" in `namespace_tuple`, `crates/moqt/src/modules/moqt/domains.rs:28`.

## Goal

An application can send a draft-18 SUBSCRIBE_NAMESPACE whose prefix has 0 fields (§2.4.1 allows 0–32) and receive every namespace.
Today `"".split('/')` gives `[""]`: one empty field, which the draft-18 encoder rejects since #1740, so the request fails.

## Plan

Open: a sentinel string, an `Option`, or a `Vec<String>` argument; draft-14 needs 1–32 fields, so it must still fail there.
