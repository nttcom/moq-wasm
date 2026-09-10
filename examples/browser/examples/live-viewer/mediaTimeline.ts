const MICROS_PER_MILLI = 1_000
const MILLIS_PER_SECOND = 1_000
const SECONDS_PER_MINUTE = 60
const SECONDS_PER_HOUR = 3_600

export type MediaTimelineRecord = {
  presentationTimeMs: number
  groupId: bigint
  objectId: bigint
  encodedAtMs: number
}

/// draft-ietf-moq-msf-00 §7.1: a media timeline is an array of records whose
/// ordinal positions hold the media presentation timestamp in milliseconds, the
/// MOQT Location as `[group id, object id]`, and the wallclock time the media
/// was encoded at in milliseconds since the Unix epoch.
export function parseMediaTimeline(document: string): MediaTimelineRecord[] {
  const parsed: unknown = JSON.parse(document)
  if (!Array.isArray(parsed)) {
    return []
  }

  return parsed.flatMap((entry) => {
    const record = readRecord(entry)
    return record ? [record] : []
  })
}

function readRecord(entry: unknown): MediaTimelineRecord | undefined {
  if (!Array.isArray(entry) || !Array.isArray(entry[1])) {
    return undefined
  }

  const [presentationTimeMs, location, encodedAtMs] = entry as [unknown, unknown[], unknown]
  const [groupId, objectId] = location
  if (![presentationTimeMs, groupId, objectId, encodedAtMs].every(isFiniteNumber)) {
    return undefined
  }

  return {
    presentationTimeMs: presentationTimeMs as number,
    groupId: BigInt(groupId as number),
    objectId: BigInt(objectId as number),
    encodedAtMs: encodedAtMs as number
  }
}

function isFiniteNumber(value: unknown): boolean {
  return typeof value === 'number' && Number.isFinite(value)
}

export class MediaTimeline {
  private records: MediaTimelineRecord[] = []

  replace(document: string): void {
    this.records = parseMediaTimeline(document).sort(
      (left, right) => left.presentationTimeMs - right.presentationTimeMs
    )
  }

  reset(): void {
    this.records = []
  }

  /// The bridge stamps a group's LOC capture timestamp and its timeline record
  /// with the same encode wallclock, so a capture time resolves to a
  /// presentation time by offsetting from the newest record at or before it.
  /// The offset is what keeps this working across renditions, whose group ids
  /// are numbered independently of the track the records point at.
  elapsedMsAt(captureMicros: number): number | undefined {
    const captureMs = captureMicros / MICROS_PER_MILLI
    const anchor = this.recordAtOrBefore(captureMs) ?? this.records[0]
    if (!anchor) {
      return undefined
    }
    return Math.max(0, anchor.presentationTimeMs + (captureMs - anchor.encodedAtMs))
  }

  private recordAtOrBefore(captureMs: number): MediaTimelineRecord | undefined {
    return this.records.filter((record) => record.encodedAtMs <= captureMs).pop()
  }
}

export function formatElapsed(millis: number): string {
  const totalSeconds = Math.floor(millis / MILLIS_PER_SECOND)
  const hours = Math.floor(totalSeconds / SECONDS_PER_HOUR)
  const minutes = Math.floor(totalSeconds / SECONDS_PER_MINUTE) % SECONDS_PER_MINUTE
  const seconds = totalSeconds % SECONDS_PER_MINUTE
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${minutes}:${pad(seconds)}`
}

function pad(value: number): string {
  return String(value).padStart(2, '0')
}
