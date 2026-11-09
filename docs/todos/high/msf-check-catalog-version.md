# [S] Leave out a catalog whose `version` is not one the reader understands

Next: in `crates/msf/src/catalog.rs:15` (`Catalog`), decide where a reader compares `version` with `MSF_VERSION` (`crates/msf/src/catalog.rs:11`, `crates/wasm/js/src/media/catalog.ts:7`).

## Goal

draft-ietf-moq-msf-01 §5.1.1: "A subscriber MUST NOT attempt to parse a catalog version which it does not understand." Today the recorder, live-translate and the browser read any catalog.
Unit tests with `"version": "draft-03"` show each of them leave that catalog out.

## Plan

Open: a delta update (§5.6.4) carries no `version`, so it takes the one of its group's independent catalog; whether the browser warns or throws.
