export type EventKind = 'chat' | 'transcript'

/// `at` is the wallclock time in Unix microseconds, the clock of the LOC
/// capture timestamps.
export type TimelineEvent = { at: number; kind: EventKind; speaker: string; text: string }

/// The kind of each `eventType` (draft-ietf-moq-msf-01 §5.2.5) the page shows.
export const EVENT_KINDS: Readonly<Record<string, EventKind>> = {
  'com.skyway.chat.v1': 'chat',
  'com.skyway.transcript.v1': 'transcript'
}

/// How long a transcribed utterance stays on its speaker's tile.
const CAPTION_US = 4_000_000

/// The records of one event timeline object (§8.1), indexed by wallclock `t`
/// in milliseconds. A chat record names no speaker; its speaker is the
/// participant whose broadcast carries the chat.
export function eventRecords(payload: Uint8Array, kind: EventKind, participant: string): TimelineEvent[] {
  const records: unknown = JSON.parse(new TextDecoder().decode(payload))
  if (!Array.isArray(records)) throw new Error('Invalid event timeline')
  return records.map((record) => {
    if (
      !Number.isSafeInteger(record?.t) ||
      !Number.isSafeInteger(record.t * 1000) ||
      typeof record?.data?.text !== 'string'
    ) {
      throw new Error('Event has no source time or text')
    }
    return {
      at: record.t * 1000,
      kind,
      speaker: typeof record.data.speaker === 'string' ? record.data.speaker : participant,
      text: record.data.text
    }
  })
}

/// Keeps one event per time and speaker, and per text for a chat: a group of
/// an event timeline repeats the records before it (§8.3), and a later
/// transcript record at the same time corrects the earlier one.
export function mergeEvents(events: TimelineEvent[]): TimelineEvent[] {
  const merged = new Map<string, TimelineEvent>()
  for (const event of events) {
    const key = JSON.stringify([event.kind, event.at, event.speaker, event.kind === 'chat' ? event.text : ''])
    merged.set(key, event)
  }
  return [...merged.values()].sort((a, b) => a.at - b.at)
}

export function captionAt(events: TimelineEvent[], speaker: string, at: number): TimelineEvent | undefined {
  return events.findLast(
    (event) => event.kind === 'transcript' && event.speaker === speaker && event.at <= at && at - event.at < CAPTION_US
  )
}
