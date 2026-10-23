import { useEffect, useMemo, useRef, useState } from 'react'
import { type BotRoomEnd, BotRoomRequest } from '../../../../utils/botRoom'
import { resolveServiceUrl } from '../../../../utils/serviceUrl'
import { type TranscriberStatus, type TranscriptRecord, freshCaptions, transcriptRecordKey } from '../media/transcript'
import type { LocalSession } from '../session/localSession'
import { TRANSCRIBER_AVAILABLE } from '../session/trackNamespace'

const TRANSCRIBER_SERVICE = {
  param: 'transcriberUrl',
  local: 'http://127.0.0.1:8100',
  cloud: 'https://moq-meeting-transcriber.moqt.research.skyway.io'
}
const TRANSCRIBER_END_TEXT: Record<BotRoomEnd, string> = {
  full: 'no transcriber free',
  closed: 'transcriber closed',
  expired: 'transcriber left after an hour',
  failed: 'transcriber stopped',
  stopped: 'stopped by a participant'
}

/// The recognizers the deployed transcriber offers (docs/architecture/ai-bots/meeting-transcriber.md).
export const ASR_CHOICES = [
  { asr: 'whisper', label: 'Whisper (CPU)' },
  { asr: 'granite-speech-5-470m', label: 'Granite 470m (CPU)' },
  { asr: 'qwen3-asr', label: 'Qwen3-ASR (GPU)' },
  { asr: 'granite-speech-5-2b', label: 'Granite 2b (GPU)' }
] as const

export function asrLabel(asr: string): string {
  return ASR_CHOICES.find((choice) => choice.asr === asr)?.label ?? asr
}

export type CaptionsControls = {
  /// Whether the transcriber is in the room or this page has asked it in.
  active: boolean
  /// Whether this page shows captions on the tiles: while `active`, unless hidden with `show(false)`.
  on: boolean
  /// Asks the transcriber in with `asr`; a transcriber already in the room keeps its own.
  start: (asr: string) => void
  /// Stops the transcriber for everyone in the room.
  stop: () => void
  /// Affects this page's tile captions only.
  show: (shown: boolean) => void
  /// `undefined` until the transcriber's status arrives and after it leaves.
  status: TranscriberStatus | undefined
  /// Why the transcriber left while this page held it.
  error: string | undefined
  /// The room's transcript while the transcriber is in the room, whether or not `on`.
  records: TranscriptRecord[]
  /// `freshCaptions` of `records` while `on`; empty otherwise.
  captions: Map<string, string>
}

/// `null` in a room the transcriber does not serve.
export function useCaptions(session: LocalSession, transcriberPresent: boolean): CaptionsControls | null {
  const [asr, setAsr] = useState<string>()
  // Set when this page's hold ends, so that the page does not hold the transcriber again before the transcriber has left the room.
  const [ended, setEnded] = useState(false)
  const [hidden, setHidden] = useState(false)
  const active = transcriberPresent || asr !== undefined
  const on = active && !hidden
  const [status, setStatus] = useState<TranscriberStatus>()
  const [error, setError] = useState<string>()
  const [records, setRecords] = useState<TranscriptRecord[]>([])
  const arrivals = useRef(new Map<string, number>())
  const [now, setNow] = useState(() => Date.now())
  // Every page holds the transcriber while it is in the room, so one page leaving does not stop it for the others.
  const heldAsr = asr ?? (transcriberPresent && !ended ? status?.asr : undefined)
  const holding = useRef<BotRoomRequest>(undefined)

  useEffect(() => {
    if (heldAsr === undefined) {
      return
    }
    const request = new BotRoomRequest((reason) => {
      setAsr(undefined)
      setEnded(true)
      setError(TRANSCRIBER_END_TEXT[reason])
    })
    request.open(resolveServiceUrl(window.location.search, session.relayUrl, TRANSCRIBER_SERVICE), session.roomName, {
      asr: heldAsr
    })
    holding.current = request
    return () => {
      // `stop` takes the request over and closes it once the server has stopped the room.
      if (holding.current === request) {
        holding.current = undefined
        request.close()
      }
    }
  }, [heldAsr, session])

  useEffect(() => {
    setEnded(false)
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
          const key = transcriptRecordKey(record)
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
    if (!transcriberPresent) {
      return
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [transcriberPresent])

  const captions = useMemo(
    () => (on ? freshCaptions(records, arrivals.current, now) : new Map<string, string>()),
    [now, on, records]
  )

  if (!TRANSCRIBER_AVAILABLE) {
    return null
  }
  return {
    active,
    on,
    start: (choice) => {
      setError(undefined)
      setEnded(false)
      setHidden(false)
      setAsr(choice)
    },
    stop: () => {
      const request = holding.current
      holding.current = undefined
      setAsr(undefined)
      setEnded(true)
      void request?.stop()
    },
    show: (shown) => setHidden(!shown),
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
