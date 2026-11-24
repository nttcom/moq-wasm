/// `live` is false for a record that came with history, from the joining FETCH.
export type TranscriptRecord = { startedAtMs: number; speaker: string; text: string; live: boolean }

/// draft-ietf-moq-msf-02 §8.4 for the transcriber's event timeline: the first
/// object of a group holds every record so far and each later object one new
/// record. The joining FETCH and the live subscription deliver the objects of
/// a group in any order, so the records follow the newest group whose first
/// object has arrived.
export class TranscriptTimeline {
  private readonly groups = new Map<bigint, Map<bigint, TranscriptRecord[]>>()
  private current: bigint | undefined

  /// Returns whether the records changed. An object that is not a JSON array
  /// is ignored, so it neither opens its group nor takes its place.
  add(groupId: bigint, objectId: bigint, payload: Uint8Array, live: boolean): boolean {
    if (this.current !== undefined && groupId < this.current) {
      return false
    }
    const objects = this.groups.get(groupId) ?? new Map<bigint, TranscriptRecord[]>()
    if (objects.has(objectId)) {
      return false
    }
    const records = parseRecords(payload, live)
    if (records === undefined) {
      return false
    }
    objects.set(objectId, records)
    this.groups.set(groupId, objects)
    if (objectId === 0n && (this.current === undefined || groupId > this.current)) {
      this.current = groupId
      for (const older of this.groups.keys()) {
        if (older < groupId) {
          this.groups.delete(older)
        }
      }
    }
    return groupId === this.current
  }

  records(): TranscriptRecord[] {
    const objects = this.current === undefined ? undefined : this.groups.get(this.current)
    if (!objects) {
      return []
    }
    return [...objects.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)).flatMap(([, records]) => records)
  }
}

/// `undefined` for an object that is not a JSON array; a record without a
/// numeric `T` and a string `data.speaker` and `data.text` is skipped.
function parseRecords(payload: Uint8Array, live: boolean): TranscriptRecord[] | undefined {
  let records: unknown
  try {
    records = JSON.parse(new TextDecoder().decode(payload))
  } catch {
    return undefined
  }
  if (!Array.isArray(records)) {
    return undefined
  }
  return records.flatMap((record) => {
    const { T, data } = record ?? {}
    return typeof T === 'number' && typeof data?.speaker === 'string' && typeof data?.text === 'string'
      ? [{ startedAtMs: T, speaker: data.speaker, text: data.text, live }]
      : []
  })
}
