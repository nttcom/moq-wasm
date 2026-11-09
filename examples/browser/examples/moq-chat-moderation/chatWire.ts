export type Location = { groupId: bigint; objectId: bigint }

export type Label = { name: string; description: string }

export type LabelProbability = { name: string; probability: number }

export type ModerationVerdict = { location: Location; labels: LabelProbability[] }

export const MAX_LABELS = 16
export const MAX_LABEL_NAME_CHARS = 20
export const MAX_LABEL_DESCRIPTION_CHARS = 100

/// Labels travel in the object, so each message is judged against the labels the page showed when
/// it was sent.
export function encodeChatMessage(text: string, labels: Label[]): Uint8Array {
  return new TextEncoder().encode(JSON.stringify({ text, labels }))
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
    const labels = record?.data?.labels
    if (
      !Array.isArray(location) ||
      location.length !== 2 ||
      !location.every(Number.isSafeInteger) ||
      !Array.isArray(labels) ||
      !labels.every((label) => typeof label?.name === 'string' && typeof label?.probability === 'number')
    ) {
      return []
    }
    return [{ location: { groupId: BigInt(location[0]), objectId: BigInt(location[1]) }, labels }]
  })
}
