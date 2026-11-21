# Design: a bot service that launches bots into rooms

## Status
Accepted

## Date
2026-11-18

## Scope
A new service `services/bot-dispatcher` that starts, stops and reports the bots of a
room, and the changes it brings to:

- `services/recorder`: `recorder record` becomes a bot; `recorder serve` keeps
  the recordings' read API (lists, files, replay, VOD) and loses start and stop.
- The Python bots (`examples/python/moq-*`) and `moq-bot-rooms`, which this
  replaces as the way a bot is started.
- `services/temporary-apps`, which becomes the app server of temporary apps.
- `crates/auth-token`, which gains the claim a launch request carries.

## Context
Two services start a bot for a room today, each in its own way:

- `recorder serve` takes `POST /recordings` with the page's client JWT as
  `Authorization: Bearer`, checks through the VTS that its `publish` claim
  covers the room, and launches `recorder record` as a Cloud Run Job with the
  token in its arguments (`--token`). The recording presents that token to the
  relay (docs/architecture/recorder/architecture.md "Recording API").
- `moq-bot-rooms` runs a bot per room while a page holds `GET /rooms/{room}`
  open, and validates a temporary app's token by refreshing it on the token
  mint (docs/architecture/ai-bots/on-demand-rooms.md).

The shape has four problems:

1. **The end user decides.** Any page whose token covers the room can start a
   recording or a bot. Whether a room is recorded, and whether a job that
   costs money runs, is the app operator's call, and a page can be edited.
2. **The bot holds the user's credential.** The recording presents the page's
   token, `publish` included, although it only subscribes. The token sits in
   the Cloud Run execution's overrides, readable by anyone who can view the
   project, and in the process list of a local launch.
3. **The bot's lifetime is the token's.** Only a temporary app's token can be
   renewed (on the temporary-apps service). A registered app's token comes from its own
   backend, which the recorder cannot ask, so a recording ends at the token's
   `exp`.
4. **The relay cannot tell the bot from the user.** Both present the same
   token.

## Decision
A launch is a server-to-server call. The browser asks its app's server; the
app's server decides and calls the bot service with a short-lived *launch
token* signed with the app's secret. The bot service launches the bot, and
the bot joins the room with a *bot token* the bot service signs for it,
scoped to what that bot needs. For temporary apps, which have no server of
their own, the temporary-apps service plays the app server.

```mermaid
sequenceDiagram
    participant P as Page
    participant A as App server (temporary-apps service for a temporary app)
    participant B as bot-dispatcher
    participant V as VTS
    participant J as Bot (Cloud Run Job)
    participant R as Relay
    P->>A: start the recorder in room X (the app's own API)
    Note over A: decides; signs a launch token for X with the app secret
    A->>B: POST /bots/recorder {namespace_prefix: X} + launch token
    B->>V: POST /verify
    V-->>B: appId, claims
    B->>B: take the (type, room) lock, write the bot as starting
    B->>J: launch (bot id only)
    J->>B: POST /bots/recorder/{id}/token (job identity)
    B-->>J: bot token for X, signed with the app secret
    J->>R: connect, SUBSCRIBE_NAMESPACE X, PUBLISH_NAMESPACE X/bots/recorder
    R->>P: PUBLISH_NAMESPACE X/bots/recorder (the page shows REC)
    loop every few seconds
        J->>B: POST /bots/recorder/{id}/heartbeat
        B-->>J: {stop: false}
    end
    Note over J: no human member for the grace period
    J->>R: close session
    J->>B: POST /bots/recorder/{id}/heartbeat {state: finished}
```

### Roles

| Party | Holds | Does |
| --- | --- | --- |
| App server | its app's secret | decides who may start a bot, signs launch tokens |
| Temporary-apps service | the temporary app secret | the app server of every temporary app (below) |
| `bot-dispatcher` | read access to the registry, the temporary app secret | verifies launch tokens through the VTS, launches bots, signs bot tokens, keeps bot state |
| Bot | its bot id and the job's own identity | joins the room with its bot token, reports its state, ends itself |

A bot never sees an app secret, a launch token or a user's token.

### The launch token
A JWT of the existing shape (`auth_token::Claims`) signed with the app's
secret, with one new claim:

| Claim | Meaning |
| --- | --- |
| `bots` | Relative namespace path under which the token may start, stop and read bots; `""` is the whole app root; absent means not allowed |

A launch token carries no `publish` or `subscribe`, so the relay grants it
nothing, and a client token carries no `bots` claim, so a page's token cannot start
a bot even if a page sends one. `bot-dispatcher` judges `iat` and `exp` with its own
`ClaimPolicy`: a launch token lives at most 5 minutes. A relay app
(`isRelay`) cannot start bots.

### The bot token
`bot-dispatcher` signs a client token for the room's app (`Claims::issued`) with the
app's secret, read from the registry through `app-registry` on every
signature, as the VTS reads it on every verification, so a rotated secret or a
deleted app takes effect at the next renewal. A temporary app's token is
signed with the temporary app secret. The claims are the bot type's:

| Type | `subscribe` | `publish` |
| --- | --- | --- |
| recorder | the room | `<room>/bots/recorder` (its presence) |
| AI bots | the room | `<room>/bots/<type>` |

A bot token lives 1 hour. Five minutes before it expires the bot asks for the
next one and sends it to the relay with the relay's in-session refresh, as
`TokenRefreshTask` and `moq_wasm.connect`'s `refresh_token` already do. `bot-dispatcher`
refuses to sign for a bot that is no longer running.

### How a bot proves it is the bot
The job is launched with its bot id and nothing secret. It calls
`POST /bots/{type}/{id}/token` and `POST /bots/{type}/{id}/heartbeat` with a Google ID token
for the bot service's URL, from the metadata server of the job's service
account. `bot-dispatcher` verifies the ID token (RS256 against Google's published keys,
`aud`, `exp`, and `email` one of the configured job service accounts) and
answers only for a bot that is running. With `--launcher local`, `bot-dispatcher`
passes the child process a random per-bot key in its environment instead,
never in its arguments, so the whole flow runs on one machine without Google
credentials. The URL of Google's keys is a setting, so the unit tests verify
ID tokens signed with a test key against that key's JWKS.

### Bots in the room
A bot publishes its presence as `<room>/bots/<type>`, the namespace the
Meeting transcriber already uses. Under a two-element room prefix
(`<app>/<room>`) that is four elements, which the Meeting's member logic never
reads as a member and no user name collides with. A page learns that a bot
joined from its PUBLISH_NAMESPACE and that it left from its
PUBLISH_NAMESPACE_DONE, so the Meeting shows REC from the room itself rather
than from polling `recorder serve`.

A **human member** is a namespace under the room that is not under one of the
reserved elements `bots` and `recordings` (the recorder's replays).

### When a bot ends
Every bot ends by itself once the room has had no human member for its type's
grace period, counted from its start too. This generalises what both bots
already do: the recorder's `AutoStop` (`--grace-seconds`) and the Python
bots' "unpublished for the grace period". The grace period differs by type
because a room that empties for a reload or a break should not split a
recording:

| Type | Grace period | Longest run |
| --- | --- | --- |
| recorder | 300 s | 2 h |
| AI bots | 30 s | 1 h |

A bot also ends on `DELETE /bots/{type}/{id}` (the next heartbeat answers
`{stop: true}`), at its longest run, and on SIGTERM. Since only publishers are
visible to a bot, a room of listeners who publish nothing counts as empty.

### Launchers
Each bot type names its launcher. The first and only one is a Cloud Run Job
execution with overridden arguments, moved from `recorder::launcher`, with the
same local child-process variant for development. The recorder's composite
job keeps launching through the same code, which therefore moves to a crate
both use instead of being copied. A type that needs to start faster than a
job (an AI bot joining a live meeting) can get another launcher later without
changing the API.

### State
`bot-dispatcher` keeps no state in memory. As `recorder serve` does, it keeps one small
JSON object per bot in a store (`gs://` or `file://`), and a lock per
`(type, room)` taken with `PutMode::Create` and released with a conditional
delete, so one room runs at most one bot of each type and any number of
instances answer alike. The object is written by `bot-dispatcher` alone, from the
launch request and from the bot's heartbeats (`starting`, `running`,
`stopping`, `finished`, `failed`). A bot that has not sent a heartbeat for
90 s, or 5 min before its first one, is reported `failed` and its lock is
released: the first execution of a job after its image changed imports the
image before the bot starts, which took almost 2 min for the AI bots' image. The recorder keeps its own recording state (members, files,
composite) in its store for `recorder serve`'s read API.

### API

| Route | Caller | Answer |
| --- | --- | --- |
| `POST /bots/{type}` `{namespace_prefix, options}` | app server | `201` the bot; `409` if the room runs that type; `503` at a limit |
| `GET /bots/{type}?namespace_prefix=` | app server | the room's latest bot of that type |
| `GET /bots/{type}/{id}` | app server | the bot |
| `DELETE /bots/{type}/{id}` | app server | `202`; the bot stops at its next heartbeat |
| `POST /bots/{type}/{id}/token` | bot | a bot token |
| `POST /bots/{type}/{id}/heartbeat` `{state, …}` | bot | `{stop}` |

The type is in the path because each type takes options of its own; a type
not in the types file, or an id of another type, is `404`. The app comes
from the launch token's `appId`, not the path. The app server's routes
require a launch token whose `bots` claim covers the prefix, judged with
`SignedToken::covers`. `options` are the type's own
(the recorder's `layout`, `composite`, `format`); the relay URL, the store and
the type's limits are server settings, so a request cannot point a bot at
another relay or bucket. Nothing is called from a browser, so the service
answers no CORS.

### Temporary apps
A temporary app has no operator and no server: whoever holds one of its
tokens is the app. The temporary-apps service, which holds the temporary app secret,
becomes the app server of all of them, so `bot-dispatcher` handles a temporary app the
same way as a registered one:

- The temporary-apps service gains the same `/bots/{type}` routes,
  which a page calls with its own token as `Authorization: Bearer`. The mint
  verifies the token as `POST /tokens/refresh` does (signed by it, temporary,
  not expired), signs a launch token whose `bots` claim is the page's
  `publish` claim, and forwards the request.
- What a temporary app may start is the temporary-apps service's policy: the bot types it
  allows (`TEMPORARY_APPS_BOT_TYPES`) and a rate limit per client address, the
  same fixed windows as `POST /tokens`.
- Anyone can create a temporary app, so anyone can still start a bot in one.
  What changes is that the decision and its limits sit in a server.

### Configuration
Bot types are a JSON object `bot-dispatcher` reads at startup from `BOT_DISPATCHER_TYPES`,
from type name to entry:

```json
{
  "recorder": {
    "launcher": { "kind": "cloud-run", "job": "projects/<p>/locations/<r>/jobs/<job>" },
    "args": ["bot"],
    "grace_seconds": 300,
    "max_seconds": 7200,
    "max_running": 5
  }
}
```

`launcher` is `{"kind": "local", "program": <path>}` for development.
Every bot gets the same token claims (above), so they are not configured.
There are no limits per app and no opening hours: a bot runs only while its
room has a member, for its type's longest run at most, a type runs at most
`max_running` bots, and the deployment's daily cost limit, which stops every
paid workload once reached, bounds what bots spend around the clock. A request
for a type not in the file is `404`.

A bot is started with the type's `args` followed by `--bot-type`, `--bot-id`,
`--bot-dispatcher-url`, `--namespace-prefix`, `--grace-seconds`, `--max-seconds` and,
when the request has them, `--options <JSON>`. A locally launched bot also
finds its key in `BOT_DISPATCHER_BOT_KEY`.

The service reads `PORT` (8098), `BOT_DISPATCHER_PUBLIC_URL` (the URL bots call back,
and the audience of their ID tokens), `BOT_DISPATCHER_VTS_URL`, `BOT_DISPATCHER_STORE_URL`,
`BOT_DISPATCHER_REGISTRY_FILE` or `BOT_DISPATCHER_REGISTRY_BUCKET` (with
`BOT_DISPATCHER_REGISTRY_OBJECT`), `BOT_DISPATCHER_TEMPORARY_APP_SECRET`, and
`BOT_DISPATCHER_JOB_SERVICE_ACCOUNT`, the comma-separated service accounts the bot
types' jobs run as, without which only locally launched bots can
authenticate. `BOT_DISPATCHER_INSECURE_SKIP_AUTH=true`, for a relay without
authorization only (local development and the E2E tests), accepts every
request for the app its room starts with, reads no registry, and answers a
bot's token call with `null`, so the bot joins with no token.

At startup, outside that mode, the service reads the registry once and fails
rather than listening with one it cannot read; a rotated registry is read again
on each token signature.

## Modules

| Module | Responsibility |
| --- | --- |
| `bot_types.rs` | `BotTypes`: the types file, each type's `JobLauncher`, arguments, grace period, longest run and `max_running`. |
| `launch_auth.rs` | `LaunchAuth`: verifies the bearer token through `auth_token::VtsClient` and requires a launch token of at most 5 minutes, not a relay's, whose `bots` claim covers the room. |
| `signer.rs` | `BotTokenSigner`: the bot token, signed with the app's secret from the registry or the temporary app secret. |
| `store.rs` | `BotStore`: the bot records, stop requests and room holders (above). |
| `bots.rs` | `Bots`: start (room holder, `max_running`, launch), the room's latest bot of a type, stop, and the bot's own token and heartbeat calls, authenticated by its key or its Google ID token (`google_cloud::IdTokenVerifier`). |
| `server.rs` | `BotDispatcherServer`: the HTTP routes on `http_server::HttpServer`, each in a `bot_dispatcher.request` span that continues the caller's trace. |

## Consequences

### Positive
- The app operator decides when a room is recorded or gets a bot, and every
  launch is authorised the same way.
- A bot holds only a token scoped to its job, renewed for as long as it runs,
  for a registered app as for a temporary one. No token is in a job's
  arguments.
- Bots are visible as bots, to the relay through their own token and to the
  room through `bots/<type>`.
- Limits and usage are counted in one place.

### Negative
- Bots run at night and on weekends too. A day whose spending reaches the
  daily cost limit stops the bots, the recordings and the GPU models together
  until the next day.
- A registered app needs a server to start bots. A page alone cannot.
- `bot-dispatcher` reads every app's secret, as the VTS does, and signs with them, as
  the console does. It becomes the third service trusted with them.
- Verifying Google ID tokens is new code (JWK parsing and RS256 with
  `aws-lc-rs`, already a dependency of `auth-token`).
- An AI bot started as a Cloud Run Job waits for the execution to start,
  about 12 s for the recorder, longer than a request to a warm service.
- The pages change twice: the recorder's start moves from `recorder serve` to
  the temporary-apps service, and the AI bot pages drop the held event stream.

## Rollout
1. `auth-token`: the `bots` claim.
2. `bot-dispatcher` with the recorder as its only type; `recorder record` takes its bot
   id, fetches its token and reports heartbeats; the recorder publishes
   `bots/recorder`.
3. Temporary-apps service `/bots/{type}`; the Meeting starts and stops the recorder through
   it and shows REC from `bots/recorder`.
4. Remove start and stop from `recorder serve`, with `--token` and
   `--temporary-apps-url` of `recorder record`.
5. The AI bots, one at a time: a `run` entry point that serves one room,
   their presence under `<room>/bots/<type>`, the common end rule, and their
   pages through the temporary-apps service. `moq-bot-rooms` goes away with the last one;
   what the bots keep of it (their calls to `bot-dispatcher`, ID tokens, telemetry) becomes `moq-bot-job`.

## Alternatives Considered

### Keep forwarding the page's token
The current shape. It needs no app server, but leaves all four problems
above.

### Let the bot join with the launch token
The bot would need no token from `bot-dispatcher`, but it cannot renew a token it did
not sign, so the app server would sign long-lived tokens, and one token would
both authorise a launch and admit to the relay.

### Let the job read the registry and sign its own token
It needs no renewal endpoint, but every job's service account reads every
app's secret, and the bot types written in Python would carry the signing
code too.

### Hand the job a renewal key in its arguments or in the store
It avoids verifying Google ID tokens, but the key sits where the token sits
today (the execution's overrides) or needs the job to read `bot-dispatcher`'s store.

### Call `bot-dispatcher` from the page for temporary apps
It leaves the temporary-apps service unchanged, but `bot-dispatcher` would keep a second,
browser-facing way in, with CORS and token refreshes at the temporary-apps service, for one kind of
app.

## Open Questions
- Whether a registered app may also opt in to starting bots from its pages'
  own tokens, for apps without a server. Left out until an app asks.
