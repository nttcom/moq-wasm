export type Location = { groupId: bigint; objectId: bigint }

export type ModerationVerdict = { location: Location; abusive: boolean }

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
