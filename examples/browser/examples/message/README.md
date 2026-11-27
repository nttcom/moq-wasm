# message

Low-level MoQT workbench for sending individual control messages and inspecting received objects and namespace events.

## Run

```shell
make relay
make browser
make chrome
```

Open `http://localhost:5173/moq-wasm/examples/message/` in one or two tabs, select a relay, and click **Connect**.

## Supported operations

| Section   | Controls                                                                                                                                     |
| --------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Namespace | `announce` (`PUBLISH_NAMESPACE`), `withdraw` (`PUBLISH_NAMESPACE_DONE`), `watchNamespaces` (`SUBSCRIBE_NAMESPACE` / `UNSUBSCRIBE_NAMESPACE`) |
| Publish   | `publishTrack` (with retention ms and optional `PUBLISH`), `write` (new group toggle), `writeGroup`                                          |
| Subscribe | `subscribe` (`largestObject` / `nextGroupStart`, `forward`, `priority`), `setForward` (`SUBSCRIBE_UPDATE`), `unsubscribe`                    |
| Fetch     | `fetch` by start/end `{groupId, objectId}` (auto-filled with the last written group)                                                         |
| Status    | `trackStatus` (`TRACK_STATUS`) returning the largest `{groupId, objectId}`                                                                   |

## E2E

```shell
node tests/browser-e2e/run-message-e2e.mjs
```
