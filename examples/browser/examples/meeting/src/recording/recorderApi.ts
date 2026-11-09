import { type ServiceUrls, resolveServiceUrl } from '../../../../utils/serviceUrl'

export type RecordingLayout = 'combined' | 'per-track'
export type RecordingFormat = 'mp4' | 'moq'

export type RecordingState = 'starting' | 'recording' | 'stopping' | 'finished' | 'failed'

export type CompositeState = 'waiting' | 'composing' | 'done' | 'skipped' | 'failed'

export interface RoomRecording {
  id: string
  namespace_prefix: string
  layout: RecordingLayout
  format: RecordingFormat
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

/// A recorder and the bearer token its requests carry, read on every request so a refreshed one is used.
export interface Recorder {
  url: string
  token: () => string | undefined
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
  recorder: Recorder,
  namespacePrefix: string,
  layout: RecordingLayout,
  composite: boolean,
  format: RecordingFormat = 'mp4'
): Promise<RoomRecording> {
  return request(recorder, '/recordings', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ namespace_prefix: namespacePrefix, layout, composite, format })
  })
}

export async function stopRecording(recorder: Recorder, id: string): Promise<RoomRecording> {
  return request(recorder, `/recordings/${encodeURIComponent(id)}/stop`, { method: 'POST' })
}

export async function listRecordings(recorder: Recorder, namespacePrefix: string): Promise<RoomRecording[]> {
  const query = new URLSearchParams({ namespace_prefix: namespacePrefix })
  const body = await request<{ recordings: RoomRecording[] }>(recorder, `/recordings?${query}`, {})
  return body.recordings
}

async function request<T>(recorder: Recorder, path: string, init: RequestInit): Promise<T> {
  const headers = new Headers(init.headers)
  const token = recorder.token()
  if (token) headers.set('Authorization', `Bearer ${token}`)
  const response = await fetch(`${recorder.url}${path}`, { ...init, headers })
  const body = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(body.error ?? `recorder answered ${response.status}`)
  }
  return body as T
}
