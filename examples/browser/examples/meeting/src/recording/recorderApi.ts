export type RecordingLayout = 'combined' | 'per-track'

export type RecordingState = 'recording' | 'stopping' | 'finished' | 'failed'

export type CompositeState = 'waiting' | 'composing' | 'done' | 'skipped' | 'failed'

export interface RoomRecording {
  id: string
  namespace_prefix: string
  layout: RecordingLayout
  state: RecordingState
  error: string | null
  members: { namespace: string; started_at_ms: number; finished: boolean; error: string | null }[]
  /// `null` unless the recording was started with `composite`.
  composite: { state: CompositeState; error: string | null } | null
}

/// `recorder serve` listens here by default.
const LOCAL_RECORDER_URL = 'http://localhost:8096'
const CLOUD_RECORDER_URL = 'https://recorder.moqt.research.skyway.io'
const LOCAL_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]'])

/// The `recorderUrl` query parameter, or the recorder that reaches the relay:
/// the local one for a local relay, the deployed one otherwise.
export function resolveRecorderUrl(search: string, relayUrl: string): string {
  const configured = new URLSearchParams(search).get('recorderUrl')?.trim()
  if (configured) {
    return configured.replace(/\/+$/, '')
  }
  try {
    return LOCAL_HOSTS.has(new URL(relayUrl).hostname) ? LOCAL_RECORDER_URL : CLOUD_RECORDER_URL
  } catch {
    return CLOUD_RECORDER_URL
  }
}

/// The recording still writing files, if any.
export function activeRecording(recordings: RoomRecording[]): RoomRecording | undefined {
  return recordings.find((recording) => recording.state === 'recording' || recording.state === 'stopping')
}

export async function startRecording(
  recorderUrl: string,
  namespacePrefix: string,
  layout: RecordingLayout,
  composite: boolean
): Promise<RoomRecording> {
  return request(`${recorderUrl}/recordings`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ namespace_prefix: namespacePrefix, layout, composite })
  })
}

export async function stopRecording(recorderUrl: string, id: string): Promise<RoomRecording> {
  return request(`${recorderUrl}/recordings/${encodeURIComponent(id)}/stop`, { method: 'POST' })
}

export async function listRecordings(recorderUrl: string, namespacePrefix: string): Promise<RoomRecording[]> {
  const query = new URLSearchParams({ namespace_prefix: namespacePrefix })
  const body = await request<{ recordings: RoomRecording[] }>(`${recorderUrl}/recordings?${query}`, {})
  return body.recordings
}

async function request<T>(url: string, init: RequestInit): Promise<T> {
  const response = await fetch(url, init)
  const body = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(body.error ?? `recorder answered ${response.status}`)
  }
  return body as T
}
