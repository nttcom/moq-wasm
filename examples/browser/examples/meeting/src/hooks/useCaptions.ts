import { useEffect, useMemo, useRef, useState } from 'react'
import { type BotRoomEnd, BotRoomRequest } from '../../../../utils/botRoom'
import { resolveServiceUrl } from '../../../../utils/serviceUrl'
import type { TranscriptRecord } from '../media/transcript'
import type { LocalSession } from '../session/localSession'
import { TRANSCRIBER_AVAILABLE } from '../session/trackNamespace'

const TRANSCRIBER_SERVICE = {
  param: 'transcriberUrl',
  local: 'http://127.0.0.1:8100',
  cloud: 'https://moq-meeting-transcriber.moqt.research.skyway.io'
}
/// How long a speaker's newest utterance stays on their tile after it arrives.
const CAPTION_MS = 5_000
/// Measured from the utterance's start, so history a joining FETCH brings is not shown as a caption.
const CAPTION_MAX_AGE_MS = 15_000
const TRANSCRIBER_END_TEXT: Record<BotRoomEnd, string> = {
  full: 'no transcriber free',
  closed: 'transcriber closed',
  expired: 'transcriber left after an hour',
  failed: 'transcriber stopped'
}

export type CaptionsControls = {
  /// Whether this page keeps the transcriber in the room and shows its text.
  on: boolean
  toggle: () => void
  /// Why the transcriber left while this page asked for it.
  error: string | undefined
  records: TranscriptRecord[]
  /// The newest utterance of each speaker, while it is fresh.
  captions: Map<string, string>
}

/// `null` in a room the transcriber does not serve.
export function useCaptions(session: LocalSession, transcriberPresent: boolean): CaptionsControls | null {
  const [on, setOn] = useState(false)
  const [error, setError] = useState<string>()
  const [records, setRecords] = useState<TranscriptRecord[]>([])
  const arrivals = useRef(new Map<string, number>())
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    if (!on) {
      return
    }
    const request = new BotRoomRequest((reason) => {
      setOn(false)
      setError(TRANSCRIBER_END_TEXT[reason])
    })
    request.open(resolveServiceUrl(window.location.search, session.relayUrl, TRANSCRIBER_SERVICE), session.roomName)
    return () => request.close()
  }, [on, session])

  useEffect(() => {
    if (!on || !transcriberPresent) {
      return
    }
    const subscribed = session.subscribeTranscript(
      (next) => {
        const receivedAt = Date.now()
        for (const record of next) {
          const key = recordKey(record)
          if (!arrivals.current.has(key)) {
            arrivals.current.set(key, receivedAt)
          }
        }
        setRecords(next)
      },
      () => setRecords([])
    )
    subscribed.catch((reason) => console.warn('[meeting][captions] transcript subscribe failed', reason))
    return () => {
      void subscribed
        .then((id) => (session.isSubscribed(id) ? session.unsubscribe(id) : undefined))
        .catch(() => undefined)
    }
  }, [on, session, transcriberPresent])

  useEffect(() => {
    if (!on) {
      setRecords([])
      arrivals.current.clear()
      return
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [on])

  const captions = useMemo(() => {
    const fresh = new Map<string, string>()
    for (const record of records) {
      const receivedAt = arrivals.current.get(recordKey(record)) ?? 0
      if (now - receivedAt < CAPTION_MS && now - record.startedAtMs < CAPTION_MAX_AGE_MS) {
        fresh.set(record.speaker, record.text)
      }
    }
    return fresh
  }, [now, records])

  if (!TRANSCRIBER_AVAILABLE) {
    return null
  }
  return {
    on,
    toggle: () => {
      setError(undefined)
      setOn((current) => !current)
    },
    error,
    records,
    captions
  }
}

function recordKey(record: TranscriptRecord): string {
  return `${record.startedAtMs}:${record.speaker}`
}
