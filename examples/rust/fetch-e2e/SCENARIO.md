# Fetch E2E Scenario

## Flow

```
alice: publish_namespace("room/alice")
bob:   subscribe("room/alice", "data")
alice: [Subscribe event] → subscribe_ok → publish group 0, 1, 2
bob:   receive live stream, track group_id
bob:   first object of group 2 received
         → relay has cached group 0 and 1 for sure
         → issue fetch A, B, C
```

## Published Data

| Group | Objects       |
|-------|---------------|
| 0     | 0, 1, 2, 3, 4 |
| 1     | 0, 1, 2, 3, 4 |
| 2     | 0, 1, 2, 3, 4 |

Payload: `"g{group}:o{object}"`

## Fetch Cases

### A: start to middle
- start: `{group:0, obj:0}`
- end:   `{group:1, obj:2}`
- expected: g0/o0-4, g1/o0-2 (8 objects)

### B: middle to middle
- start: `{group:1, obj:2}`
- end:   `{group:2, obj:3}`
- expected: g1/o2-4, g2/o0-3 (7 objects)

### C: entire group (end.obj=0 means no upper bound)
- start: `{group:1, obj:0}`
- end:   `{group:1, obj:0}`
- expected: g1/o0-4 (5 objects)
