# [M] Index active bots in bot-dispatcher instead of scanning every room holder

Next: inspect `BotStore::room_holders` (`services/bot-dispatcher/src/store.rs:169`) and `Bots::running` (`services/bot-dispatcher/src/bots.rs:370`).

## Goal

`POST /bots/{type}` enforces `max_running` without reading every room that ever ran a bot, and `bot-dispatcher` can answer an app-wide `GET /bots?state=running` without scanning other apps.
Today `BotStore` never deletes `rooms/<type>/<namespace prefix>` (`services/bot-dispatcher/src/store.rs:58-62`), so `Bots::running` reads every room holder of the launched type, in every app, on each launch that passes the room's conflict check (`services/bot-dispatcher/src/bots.rs:141`).
An app that names a room per visit grows that scan without bound, and an app server restoring state after a restart cannot list its running bots.

## Plan

Keep an index of active bots (or release a room holder once its bot ends) that both `max_running` and an app-wide listing read.
Decide how `GET /bots/{type}?namespace_prefix=` and `GET /bots?namespace_prefix=` keep reporting ended bots if holders are released, and how silent timed-out bots are pruned from the index.
