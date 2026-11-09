# [S] Cap how long a live-translate page keeps the Cloud Run instance alive

Next: decide the cap, then close the `EventSource` in `examples/browser/examples/live-translate/liveTranslate.ts:30` once it has been open that long.

## Goal

A tab left open does not keep the translator's Cloud Run instance up for days. Today the page's `/events` stream reconnects after every 3600 s request timeout, so one forgotten tab keeps the instance (about $0.1/h, $70/month) running until the tab is closed. Gemini itself is already bounded by the language time limit and the open hours.

## Plan

Close the stream after a fixed time since the page opened and show "ページを読み込み直してください" instead of reconnecting. The relay token already expires after 1 h, so that is a natural length.
