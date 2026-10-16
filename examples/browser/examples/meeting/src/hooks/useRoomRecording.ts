import { useCallback, useEffect, useState } from 'react'
import {
  type RecordingLayout,
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
  start: (layout: RecordingLayout) => Promise<void>
  stop: () => Promise<void>
}

/// `null` until the recorder has answered once, so a recorder that is not
/// running hides recording.
export function useRoomRecording(recorderUrl: string, namespacePrefix: string): RoomRecordingControls | null {
  const [reachable, setReachable] = useState(false)
  const [active, setActive] = useState<RoomRecording | undefined>()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    try {
      setActive(activeRecording(await listRecordings(recorderUrl, namespacePrefix)))
      setReachable(true)
      setError(null)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }, [namespacePrefix, recorderUrl])

  useEffect(() => {
    void refresh()
    const timer = setInterval(() => void refresh(), POLL_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [refresh])

  const run = useCallback(
    async (action: (url: string) => Promise<RoomRecording>) => {
      setBusy(true)
      try {
        const recording = await action(recorderUrl)
        setActive(activeRecording([recording]))
        setError(null)
      } catch (caught) {
        setError(caught instanceof Error ? caught.message : String(caught))
      } finally {
        setBusy(false)
      }
    },
    [recorderUrl]
  )

  const start = useCallback(
    (layout: RecordingLayout) => run((url) => startRecording(url, namespacePrefix, layout)),
    [namespacePrefix, run]
  )
  const stop = useCallback(async () => {
    const id = active?.id
    if (id) {
      await run((url) => stopRecording(url, id))
    }
  }, [active?.id, run])

  if (!reachable) {
    return null
  }
  return { active, busy, error, start, stop }
}
