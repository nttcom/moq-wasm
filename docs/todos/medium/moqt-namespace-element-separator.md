# [S] Keep a namespace element that contains `/` one element on the wire

Next: replace `join_namespace` in `crates/moqt/src/modules/client.rs:13`, which joins elements with `/`, by passing the element list through to the Track Namespace tuple.

## Goal

A client that announces `["anon", "room", "directory/filename"]` is seen by a SUBSCRIBE_NAMESPACE watcher with three elements; today the watcher gets four (found by the Python SDK review, R2-23).

## Plan

Open: whether the origin's track keys can keep `/`-joined strings if the public APIs reject an element containing `/` instead.
