import type { TranscriptRecord } from 'moq-wasm/media'

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
