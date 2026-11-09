import { useCallback, useEffect, useMemo, useState } from 'react'
import { bearerToken } from '../../../../utils/access'
import {
  type RecordingLayout,
  type RecordingFormat,
  type Recorder,
  type RoomRecording,
  activeRecording,
  listRecordings,
  startRecording,
  stopRecording
} from '../recording/recorderApi'

const POLL_INTERVAL_MS = 5_000

export const RECORDING_CHOICES: {
  id: string
  layout: RecordingLayout
  composite: boolean
  label: string
  format?: RecordingFormat
}[] = [
  { id: 'moq', layout: 'combined', composite: false, format: 'moq', label: 'Record meeting with chat and transcript' },
  { id: 'composite', layout: 'combined', composite: true, label: 'All Track in one file' },
  { id: 'combined', layout: 'combined', composite: false, label: 'User Track in one file' },
  { id: 'per-track', layout: 'per-track', composite: false, label: 'Per Track in one file' }
]

export function pendingRecordingLabel(active: RoomRecording | undefined): string | null {
  switch (active?.state) {
    case 'starting':
      return 'Starting the recording'
    case 'stopping':
      return 'Stopping the recording'
    default:
      return null
  }
}

/// A finished recording the recording player can open.
export interface RecordingReplay {
  id: string
  /// When its first member started recording, or `undefined` if none did.
  startedAtMs: number | undefined
  url: string
}

export interface RoomRecordingControls {
  /// The room's recording that still writes files, whoever started it.
  active: RoomRecording | undefined
  busy: boolean
  error: string | null
  /// Oldest first.
  replays: RecordingReplay[]
  /// `composite` also makes one video of the whole room after the recording.
  start: (layout: RecordingLayout, composite: boolean, format?: RecordingFormat) => Promise<void>
  stop: () => Promise<void>
}

/// `null` until the recorder has answered once, so a recorder that is not
/// running hides recording. `relayUrl` is the relay the player opens the
/// recording through.
export function useRoomRecording(
  recorderUrl: string,
  relayUrl: string,
  namespacePrefix: string
): RoomRecordingControls | null {
  const [reachable, setReachable] = useState(false)
  const [active, setActive] = useState<RoomRecording | undefined>()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [replays, setReplays] = useState<RecordingReplay[]>([])
  const recorder = useMemo<Recorder>(() => ({ url: recorderUrl, token: bearerToken }), [recorderUrl])

  const refresh = useCallback(async () => {
    try {
      const recordings = await listRecordings(recorder, namespacePrefix)
      setActive(activeRecording(recordings))
      setReplays(
        recordings
          // The recorder replays only the MoQ archive format.
          .filter((recording) => recording.state === 'finished' && recording.format === 'moq')
          .map((recording) => ({
            id: recording.id,
            startedAtMs: recording.members.length
              ? Math.min(...recording.members.map((member) => member.started_at_ms))
              : undefined,
            url: replayUrl(recorderUrl, relayUrl, recording.id)
          }))
      )
      setReachable(true)
      setError(null)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }, [namespacePrefix, recorder, recorderUrl, relayUrl])

  useEffect(() => {
    void refresh()
    const timer = setInterval(() => void refresh(), POLL_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [refresh])

  const run = useCallback(
    async (action: (recorder: Recorder) => Promise<RoomRecording>) => {
      setBusy(true)
      try {
        const recording = await action(recorder)
        setActive(activeRecording([recording]))
        setError(null)
      } catch (caught) {
        setError(caught instanceof Error ? caught.message : String(caught))
      } finally {
        setBusy(false)
      }
    },
    [recorder]
  )

  const start = useCallback(
    (layout: RecordingLayout, composite: boolean, format: RecordingFormat = 'mp4') =>
      run((recorder) => startRecording(recorder, namespacePrefix, layout, composite, format)),
    [namespacePrefix, run]
  )
  const stop = useCallback(async () => {
    const id = active?.id
    if (id) {
      await run((recorder) => stopRecording(recorder, id))
    }
  }, [active?.id, run])

  if (!reachable) {
    return null
  }
  return { active, busy, error, start, stop, replays }
}

function replayUrl(recorderUrl: string, relayUrl: string, id: string): string {
  const url = new URL('../recording-player/', window.location.href)
  url.searchParams.set('recorder', recorderUrl)
  url.searchParams.set('relayUrl', relayUrl)
  url.searchParams.set('id', id)
  return url.href
}
