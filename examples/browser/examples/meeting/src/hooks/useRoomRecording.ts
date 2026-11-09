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

export interface RoomRecordingControls {
  /// The room's recording that still writes files, whoever started it.
  active: RoomRecording | undefined
  busy: boolean
  error: string | null
  playbackUrl: string | undefined
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
  const [playbackUrl, setPlaybackUrl] = useState<string>()
  const recorder = useMemo<Recorder>(() => ({ url: recorderUrl, token: bearerToken }), [recorderUrl])

  const refresh = useCallback(async () => {
    try {
      const recordings = await listRecordings(recorder, namespacePrefix)
      setActive(activeRecording(recordings))
      const latest = recordings.filter((recording) => recording.state === 'finished').at(-1)
      const url = new URL('../recording-player/', window.location.href)
      if (latest) {
        url.searchParams.set('recorder', recorderUrl)
        url.searchParams.set('relayUrl', relayUrl)
        url.searchParams.set('id', latest.id)
      }
      setPlaybackUrl(latest ? url.href : undefined)
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
  return { active, busy, error, start, stop, playbackUrl }
}
