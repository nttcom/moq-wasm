/// `live` is false for a record that came with history, from the joining FETCH.
export type TranscriptRecord = { startedAtMs: number; speaker: string; text: string; live: boolean }

/// How long a live record stays in its speaker's caption after it arrives.
const CAPTION_MS = 5_000
/// The longest caption a tile shows; a longer one loses its start.
const CAPTION_MAX_CHARS = 60

export function transcriptRecordKey(record: TranscriptRecord): string {
  return `${record.startedAtMs}:${record.speaker}`
}

/// The tile caption of each speaker: their records that arrived within `CAPTION_MS` of `now`, joined.
/// `arrivals` holds when each live record arrived, by `transcriptRecordKey`; a record missing from it is left out.
export function freshCaptions(
  records: TranscriptRecord[],
  arrivals: Map<string, number>,
  now: number
): Map<string, string> {
  const fresh = new Map<string, string>()
  for (const record of records) {
    const receivedAt = arrivals.get(transcriptRecordKey(record))
    if (receivedAt !== undefined && now - receivedAt < CAPTION_MS) {
      const earlier = fresh.get(record.speaker)
      const caption = earlier === undefined ? record.text : `${earlier} ${record.text}`
      fresh.set(record.speaker, caption.slice(-CAPTION_MAX_CHARS))
    }
  }
  return fresh
}

/// The transcriber's `status` track: its recognizer, and `starting` until that answers.
export type TranscriberStatus = { asr: string; state: 'starting' | 'ready' }

export function parseTranscriberStatus(payload: Uint8Array): TranscriberStatus | undefined {
  const { asr, state } = JSON.parse(new TextDecoder().decode(payload)) ?? {}
  return typeof asr === 'string' && (state === 'starting' || state === 'ready') ? { asr, state } : undefined
}

/// draft-ietf-moq-msf-01 §8.3 for the transcriber's event timeline: the first
/// object of a group holds every record so far and each later object one new
/// record. The joining FETCH and the live subscription deliver the objects of
/// a group in any order, so the records follow the newest group whose first
/// object has arrived.
export class TranscriptTimeline {
  private readonly groups = new Map<bigint, Map<bigint, TranscriptRecord[]>>()
  private current: bigint | undefined

  /// Returns whether the records changed.
  add(groupId: bigint, objectId: bigint, payload: Uint8Array, live: boolean): boolean {
    if (this.current !== undefined && groupId < this.current) {
      return false
    }
    const objects = this.groups.get(groupId) ?? new Map<bigint, TranscriptRecord[]>()
    if (objects.has(objectId)) {
      return false
    }
    objects.set(objectId, parseRecords(payload, live))
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

function parseRecords(payload: Uint8Array, live: boolean): TranscriptRecord[] {
  const records: unknown = JSON.parse(new TextDecoder().decode(payload))
  if (!Array.isArray(records)) {
    return []
  }
  return records.flatMap((record) => {
    const { t, data } = record ?? {}
    return typeof t === 'number' && typeof data?.speaker === 'string' && typeof data?.text === 'string'
      ? [{ startedAtMs: t, speaker: data.speaker, text: data.text, live }]
      : []
  })
}
