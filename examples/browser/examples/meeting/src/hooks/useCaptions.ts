import { useEffect, useMemo, useRef, useState } from 'react'
import { latestBot, startBot, stopBot } from '../../../../utils/roomBots'
import { type TranscriberStatus, type TranscriptRecord, freshCaptions, transcriptRecordKey } from '../media/transcript'
import type { LocalSession } from '../session/localSession'
import { TRANSCRIBER_AVAILABLE, TRANSCRIBER_BOT_TYPE } from '../session/trackNamespace'

/// How often a page that started the transcriber asks `bots` whether it failed before joining the room.
const STARTING_POLL_INTERVAL_MS = 5_000

/// The recognizers the deployed transcriber offers (docs/architecture/ai-bots/meeting-transcriber.md).
export const ASR_CHOICES = [
  { asr: 'whisper', label: 'Whisper (CPU)' },
  { asr: 'granite-speech-5-470m', label: 'Granite 470m (CPU)' },
  { asr: 'qwen3-asr', label: 'Qwen3-ASR (GPU)' },
  { asr: 'granite-speech-5-2b', label: 'Granite 2b (GPU)' },
  { asr: 'haas-ja', label: 'Haas-ja (GPU)' }
] as const

export type CaptionsControls = {
  /// Whether the transcriber is in the room or this page has started it.
  active: boolean
  /// Whether this page shows captions on the tiles: while `active`, unless hidden with `show(false)`.
  on: boolean
  /// Starts the transcriber with `asr`; a transcriber already in the room keeps its own.
  start: (asr: string) => void
  /// Stops the transcriber for everyone in the room.
  stop: () => void
  /// Affects this page's tile captions only.
  show: (shown: boolean) => void
  /// `undefined` until the transcriber's status arrives and after it leaves.
  status: TranscriberStatus | undefined
  /// Why this page failed to start or stop the transcriber.
  error: string | undefined
  /// The room's transcript while the transcriber is in the room, whether or not `on`.
  records: TranscriptRecord[]
  /// `freshCaptions` of `records` while `on`; empty otherwise.
  captions: Map<string, string>
}

/// `null` in a room the transcriber does not serve. The transcriber is a bot of the room
/// (utils/roomBots.ts), which any page starts and stops; whether it is in the room comes from its
/// namespace, `transcriberPresent`.
export function useCaptions(session: LocalSession, transcriberPresent: boolean): CaptionsControls | null {
  // The bot this page started, until it joins the room or ends.
  const [startingId, setStartingId] = useState<string>()
  const [hidden, setHidden] = useState(false)
  const active = transcriberPresent || startingId !== undefined
  const on = active && !hidden
  const [status, setStatus] = useState<TranscriberStatus>()
  const [error, setError] = useState<string>()
  const [records, setRecords] = useState<TranscriptRecord[]>([])
  const arrivals = useRef(new Map<string, number>())
  const [now, setNow] = useState(() => Date.now())
  const namespacePrefix = session.trackNamespacePrefix.join('/')

  useEffect(() => {
    if (transcriberPresent) {
      setStartingId(undefined)
    }
  }, [transcriberPresent])

  useEffect(() => {
    if (startingId === undefined) {
      return
    }
    const timer = window.setInterval(() => {
      latestBot(TRANSCRIBER_BOT_TYPE, namespacePrefix)
        .then((bot) => {
          if (bot?.id !== startingId || bot.state === 'failed' || bot.state === 'finished') {
            setStartingId(undefined)
            setError(bot?.state === 'failed' ? 'transcriber failed to start' : undefined)
          }
        })
        .catch((caught) => console.warn('[meeting][captions] cannot read the transcriber', caught))
    }, STARTING_POLL_INTERVAL_MS)
    return () => window.clearInterval(timer)
  }, [namespacePrefix, startingId])

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
    start: (asr) => {
      setError(undefined)
      setHidden(false)
      startBot(TRANSCRIBER_BOT_TYPE, namespacePrefix, { asr })
        .then((bot) => setStartingId(bot.id))
        .catch((caught) => setError(errorText(caught)))
    },
    stop: () => {
      setStartingId(undefined)
      latestBot(TRANSCRIBER_BOT_TYPE, namespacePrefix)
        .then((bot) => (bot ? stopBot(TRANSCRIBER_BOT_TYPE, bot.id) : undefined))
        .catch((caught) => setError(errorText(caught)))
    },
    show: (shown) => setHidden(!shown),
    status,
    error,
    records,
    captions
  }
}

function errorText(caught: unknown): string {
  return caught instanceof Error ? caught.message : String(caught)
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
