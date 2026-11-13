# [XS] Close a draft-14 session on an unknown control message type

Next: in `crates/moqt/src/modules/moqt/version/draft14/control_message_receive_task.rs:76`, close the connection on a decode error instead of logging at DEBUG and leaving the loop.

## Goal

draft-14 §9: "An endpoint that receives an unknown message type MUST close the session."
Reproduced with Message Type 0x3f: the session stays open, the sender gets STOP_SENDING (code 0) on the control stream and its later control writes fail, and the receiving application gets no event.

## Plan

Open: the sentence names no error code; draft-18's path closes with PROTOCOL_VIOLATION.
