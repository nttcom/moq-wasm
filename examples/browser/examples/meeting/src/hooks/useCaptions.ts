import { useEffect, useMemo, useRef, useState } from 'react'
import { type BotRoomEnd, BotRoomRequest } from '../../../../utils/botRoom'
import { resolveServiceUrl } from '../../../../utils/serviceUrl'
import type { TranscriberStatus, TranscriptRecord } from '../media/transcript'
import type { LocalSession } from '../session/localSession'
import { TRANSCRIBER_AVAILABLE } from '../session/trackNamespace'

const TRANSCRIBER_SERVICE = {
  param: 'transcriberUrl',
  local: 'http://127.0.0.1:8100',
  cloud: 'https://moq-meeting-transcriber.moqt.research.skyway.io'
}
/// How long a speaker's newest live utterance stays on their tile after it arrives.
const CAPTION_MS = 5_000
const TRANSCRIBER_END_TEXT: Record<BotRoomEnd, string> = {
  full: 'no transcriber free',
  closed: 'transcriber closed',
  expired: 'transcriber left after an hour',
  failed: 'transcriber stopped'
}

/// The recognizers the deployed transcriber offers (docs/architecture/ai-bots/meeting-transcriber.md).
export const ASR_CHOICES = [
  { asr: 'whisper', label: 'Whisper (CPU)' },
  { asr: 'qwen3-asr', label: 'Qwen3-ASR (GPU)' },
  { asr: 'moss-transcribe-diarize', label: 'MOSS (GPU)' }
] as const

export function asrLabel(asr: string): string {
  return ASR_CHOICES.find((choice) => choice.asr === asr)?.label ?? asr
}

export type CaptionsControls = {
  /// Whether this page keeps the transcriber in the room and shows captions on the tiles.
  on: boolean
  /// Asks the transcriber in with `asr`; a transcriber already in the room keeps its own.
  start: (asr: string) => void
  stop: () => void
  /// `undefined` until the transcriber's status arrives and after it leaves.
  status: TranscriberStatus | undefined
  /// Why the transcriber left while this page asked for it.
  error: string | undefined
  /// The room's transcript while the transcriber is in the room, whether or not `on`.
  records: TranscriptRecord[]
  /// The newest utterance of each speaker, while it is fresh.
  captions: Map<string, string>
}

/// `null` in a room the transcriber does not serve.
export function useCaptions(session: LocalSession, transcriberPresent: boolean): CaptionsControls | null {
  const [asr, setAsr] = useState<string>()
  const on = asr !== undefined
  const [status, setStatus] = useState<TranscriberStatus>()
  const [error, setError] = useState<string>()
  const [records, setRecords] = useState<TranscriptRecord[]>([])
  const arrivals = useRef(new Map<string, number>())
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    if (asr === undefined) {
      return
    }
    const request = new BotRoomRequest((reason) => {
      setAsr(undefined)
      setError(TRANSCRIBER_END_TEXT[reason])
    })
    request.open(resolveServiceUrl(window.location.search, session.relayUrl, TRANSCRIBER_SERVICE), session.roomName, {
      asr
    })
    return () => request.close()
  }, [asr, session])

  useEffect(() => {
    if (!transcriberPresent) {
      setStatus(undefined)
      return
    }
    return holdSubscription(session, session.subscribeTranscriberStatus(setStatus), 'transcriber status')
  }, [session, transcriberPresent])

  useEffect(() => {
    if (!transcriberPresent) {
      setRecords([])
      arrivals.current.clear()
      return
    }
    const subscribed = session.subscribeTranscript(
      (next) => {
        const receivedAt = Date.now()
        for (const record of next) {
          const key = recordKey(record)
          if (record.live && !arrivals.current.has(key)) {
            arrivals.current.set(key, receivedAt)
          }
        }
        setRecords(next)
      },
      () => setRecords([])
    )
    return holdSubscription(session, subscribed, 'transcript')
  }, [session, transcriberPresent])

  useEffect(() => {
    if (!on) {
      return
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [on])

  const captions = useMemo(() => {
    const fresh = new Map<string, string>()
    if (!on) {
      return fresh
    }
    for (const record of records) {
      const receivedAt = arrivals.current.get(recordKey(record)) ?? 0
      if (now - receivedAt < CAPTION_MS) {
        fresh.set(record.speaker, record.text)
      }
    }
    return fresh
  }, [now, on, records])

  if (!TRANSCRIBER_AVAILABLE) {
    return null
  }
  return {
    on,
    start: (choice) => {
      // The clock stops while captions are off, so records that arrived meanwhile would read as fresh.
      setNow(Date.now())
      setError(undefined)
      setAsr(choice)
    },
    stop: () => setAsr(undefined),
    status,
    error,
    records,
    captions
  }
}

/// The effect's cleanup: unsubscribes whether or not the SUBSCRIBE has been answered yet.
function holdSubscription(session: LocalSession, subscribed: Promise<bigint>, track: string): () => void {
  subscribed.catch((reason) => console.warn(`[meeting][captions] ${track} subscribe failed`, reason))
  return () => {
    void subscribed
      .then((id) => (session.isSubscribed(id) ? session.unsubscribe(id) : undefined))
      .catch(() => undefined)
  }
}

function recordKey(record: TranscriptRecord): string {
  return `${record.startedAtMs}:${record.speaker}`
}
