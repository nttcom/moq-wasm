# [M] Play review sound through the page's AudioContext

Next: replace the context's `suspend()` in `ReviewClock.applyPauseToContext` (`crates/wasm/js/src/media/player/reviewClock.ts:145`) with a flush of every follower's sound.

## Goal

With `LivePlayerOptions.audioContext`, a rewind or a `PlaybackGroup` plays through the page's context and follows its `setSinkId`; today only live LOC sound does.

## Plan

`ReviewClock` suspends its own context to pause, which a shared context cannot take, so pausing flushes and reschedules instead.
Open: which context a `PlaybackGroup` uses when its players were given different ones.
