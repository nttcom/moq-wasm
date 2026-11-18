# whiteboard

A whiteboard the people in one room draw on together. Open the page, choose a
relay and press 参加; anyone who opens the same URL (its `?room=` names the
room) draws on the same board.

## Tracks

Each participant announces `anon/whiteboard/<room>/<participant>` and publishes
one track, `board`, and follows the others with `watchNamespaces` on
`anon/whiteboard/<room>`. Each object is one JSON operation (`board.ts`):

| Operation  | Meaning                                            |
| ---------- | -------------------------------------------------- |
| `snapshot` | The whole board as the writer sees it              |
| `stroke`   | A new stroke with its first point                  |
| `points`   | Points of a stroke being drawn, from a point index |
| `erase`    | Stroke IDs the eraser touched                      |
| `clear`    | Removes every stroke stamped before it             |

## Late joiners

Every group starts with a `snapshot` of the whole board, followed by the
writer's own operations. A participant starts a new group after 100 of its own
operations, and whenever another participant leaves.

A late joiner subscribes to every participant's track with
`subscribeCurrentGroup`: the joining FETCH delivers the current group from its
snapshot, then the subscription goes on live. It applies every snapshot and
operation it receives; none is picked over another, since they all merge:

- strokes are a set keyed by ID, and of two copies of one stroke the one with
  more points wins;
- erased IDs are a set, kept so a stroke arriving after its erase stays hidden;
- the clear point is the latest `clear`.

A stroke's ID is its Lamport stamp (`clock@participant`). A participant stamps
its operations after every stamp it has seen, so a stroke drawn after seeing a
clear survives it, and every page orders concurrent ones the same way.

The current group of a participant holds all of that participant's strokes, so
the groups of the people present cover theirs. A participant who leaves takes
its track along; the new group every remaining participant starts then carries
its strokes. When everyone has left, the board is gone.

The track is published with a one-hour retention, so the publisher still
answers the FETCH for a group written long ago while the board sits idle.
