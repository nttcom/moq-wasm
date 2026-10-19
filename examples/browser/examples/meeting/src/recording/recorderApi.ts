import { type ServiceUrls, resolveServiceUrl } from '../../../../utils/serviceUrl'

export type RecordingLayout = 'combined' | 'per-track'

export type RecordingState = 'starting' | 'recording' | 'stopping' | 'finished' | 'failed'

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

const RECORDER_URLS: ServiceUrls = {
  param: 'recorderUrl',
  /// `recorder serve` listens here by default.
  local: 'http://localhost:8096',
  cloud: 'https://recorder.moqt.research.skyway.io'
}

export function resolveRecorderUrl(search: string, relayUrl: string): string {
  return resolveServiceUrl(search, relayUrl, RECORDER_URLS)
}

export function activeRecording(recordings: RoomRecording[]): RoomRecording | undefined {
  return recordings.find(
    (recording) => recording.state === 'starting' || recording.state === 'recording' || recording.state === 'stopping'
  )
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
