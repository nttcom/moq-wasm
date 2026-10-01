export type Location = { groupId: bigint; objectId: bigint }

export type ModerationVerdict = { location: Location; abusive: boolean }

export const MAX_CHAT_TEXT_LENGTH = 1000
const NON_FINAL_STORED_BLOCK_HEADER = 0x00
const STORED_BLOCK_HEADER_LENGTH = 5

/// pipecat's MOQTransport reads the chat track as a moq-rs compressed JSON stream: each group is
/// one raw DEFLATE stream sync-flushed after every frame, with the flush's fixed `00 00 ff ff`
/// tail left off the wire. Stored (uncompressed) blocks are valid DEFLATE, so a record needs no
/// compressor; the trailing header byte opens the empty block the reader completes with that tail.
/// A text of at most MAX_CHAT_TEXT_LENGTH UTF-16 units stays under a stored block's 65535 bytes.
export function encodeChatRecord(text: string, location: Location): Uint8Array {
  const json = JSON.stringify({ text, location: [Number(location.groupId), Number(location.objectId)] })
  const data = new TextEncoder().encode(json)
  const frame = new Uint8Array(STORED_BLOCK_HEADER_LENGTH + data.length + 1)
  frame.set([
    NON_FINAL_STORED_BLOCK_HEADER,
    data.length & 0xff,
    data.length >> 8,
    ~data.length & 0xff,
    (~data.length >> 8) & 0xff
  ])
  frame.set(data, STORED_BLOCK_HEADER_LENGTH)
  frame[frame.length - 1] = NON_FINAL_STORED_BLOCK_HEADER
  return frame
}

/// draft-ietf-moq-msf-01 §8.1: an event timeline object is a JSON array of records indexed by
/// `l`, the [group id, object id] of the chat object they judge.
export function parseModerationVerdicts(payload: Uint8Array): ModerationVerdict[] {
  const records: unknown = JSON.parse(new TextDecoder().decode(payload))
  if (!Array.isArray(records)) {
    return []
  }
  return records.flatMap((record) => {
    const location = record?.l
    const abusive = record?.data?.abusive
    if (
      !Array.isArray(location) ||
      location.length !== 2 ||
      !location.every(Number.isSafeInteger) ||
      typeof abusive !== 'boolean'
    ) {
      return []
    }
    return [{ location: { groupId: BigInt(location[0]), objectId: BigInt(location[1]) }, abusive }]
  })
}
