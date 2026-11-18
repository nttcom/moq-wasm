# [S] Give the examples' tab controls the keyboard of tabs, or make them toggle buttons

Next: pick the pattern for `examples/browser/examples/onvif/index.html:72` (PTZ Controller / Commands).

## Goal

Every `role="tablist"` moves with the arrow keys (roving tabindex), or is a `role="group"` of `aria-pressed` buttons like the example list's switches.
Today onvif `:72`, `observability/src/ChartDrawer.tsx:48` and `utils/relayPicker.ts:142` are tablists without arrow keys, and `recording-player/index.html:71` puts `aria-selected` on plain buttons.
