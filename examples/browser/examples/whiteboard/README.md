# whiteboard

Collaborative whiteboard over MoQT. Open the page, pick a relay, and click **参加**; anyone opening the same `?room=` URL shares the board.

## Tracks

Each participant announces `anon/whiteboard/<room>/<participant>`, publishes `board`, and discovers peers via `watchNamespaces` on `anon/whiteboard/<room>`. Each object is a JSON operation (`board.ts`):

| Operation  | Meaning                                    |
| ---------- | ------------------------------------------ |
| `snapshot` | Full board state as seen by the sender     |
| `stroke`   | New stroke with its first point            |
| `points`   | Additional points from a given point index |
| `erase`    | Stroke IDs erased                          |
| `clear`    | Removes all strokes stamped earlier        |

## Late joiners & merging

Each group begins with a `snapshot`, followed by the sender's operations; the track retains objects (including snapshots) for 1 hour so idle boards still answer `FETCH`. A new group starts every 100 local operations and whenever another participant leaves (carrying over the departing peer's strokes; when everyone leaves, the board is gone).

Late joiners subscribe with `subscribeCurrentGroup` (joining `FETCH` from the group's `snapshot`, then live) and merge every peer's state commutatively:

- **Strokes**: keyed by Lamport ID (`clock@participant`); more points wins. Each participant stamps operations after every stamp it has seen, so strokes drawn after seeing a `clear` survive and all peers order concurrent strokes identically.
- **Erases**: grow-only set of erased IDs so late-arriving strokes stay hidden.
- **Clear**: latest Lamport timestamp wins; strokes stamped after seeing a `clear` survive.
