# [XS] Refuse an independent catalog without a tracks array in msf

Next: in `crates/msf/src/catalog.rs:107` `Catalog::parse`, return `CatalogParseError::Invalid` when `tracks` is `None`, and add a test for `{"version":"draft-02"}` and `"tracks":null`.

## Goal

`Catalog::parse` refuses `{"version":"draft-02","tracks":null}` and a catalog without `tracks`, which draft-ietf-moq-msf-02 §5.1.4 requires (Array) of an independent catalog; today it accepts both.
`parse_catalog`, the Python SDK's `Catalog.parse` and the browser SDK pass them on.

## Plan

`tracks` stays `Option` in the struct only if a delta update needs it; `parse` already refuses delta updates.
