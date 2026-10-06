export type SessionPeer = 'client' | 'relay' | 'stats_publisher'

export interface ProcessStats {
  rss_bytes: number | null
  cache_tracks: number
  cache_objects: number
  cache_payload_bytes: number
}

export interface SessionStats {
  session_id: number
  peer: SessionPeer
  app_id: string
  remote_address: string | null
  local_ip: string | null
  dialed_relay_id: string | null
  rtt_us: number
  current_mtu: number
  sent_bytes: number
  sent_packets: number
  lost_packets: number
  lost_bytes: number
  cwnd: number
  congestion_events: number
  sent_stream_data_blocked: number
  sent_data_blocked: number
  received_stop_sending: number
  received_bytes: number
  received_stream_data_blocked: number
  received_data_blocked: number
  received_reset_stream: number
}

export interface TrackStats {
  namespace: string
  name: string
  publisher_session_id: number
  objects_received: number
  bytes_received: number
  subgroups_aborted: number
  max_arrival_gap_since_last_snapshot_us: number
}

export interface SubscriptionStats {
  namespace: string
  name: string
  publisher_session_id: number
  subscriber_session_id: number
  request_id: number
  forward: boolean
  objects_sent: number
  bytes_sent: number
  streams_opened: number
  streams_reset: number
  lag_behind_newest_received_us: number
}

export interface RelaySnapshot {
  relay_id: string
  timestamp_ms: number
  process: ProcessStats
  sessions: SessionStats[]
  tracks: TrackStats[]
  subscriptions: SubscriptionStats[]
}

export type SeriesTarget =
  | { target: 'process' }
  | { target: 'relay' }
  | { target: 'session'; session_id: number }
  | { target: 'track'; publisher_session_id: number; namespace: string; name: string }
  | { target: 'subscription'; subscriber_session_id: number; request_id: number }

export interface Series {
  t: number[]
  series: Record<string, (number | null)[]>
}

const API_BASE = new URLSearchParams(window.location.search).get('api') ?? 'http://localhost:8095'

async function getJson<T>(path: string, params: Record<string, string | number>): Promise<T> {
  const query = new URLSearchParams(Object.entries(params).map(([key, value]) => [key, String(value)]))
  const response = await fetch(`${API_BASE}${path}?${query}`)
  if (!response.ok) {
    throw new Error(`${path} answered ${response.status}: ${await response.text()}`)
  }
  return (await response.json()) as T
}

export function fetchSnapshots(atMs?: number): Promise<RelaySnapshot[]> {
  return getJson('/api/snapshots', atMs === undefined ? {} : { at: atMs })
}

export function fetchSeries(
  relayId: string,
  target: SeriesTarget,
  fromMs: number,
  toMs: number,
  points: number
): Promise<Series> {
  return getJson('/api/series', { relay_id: relayId, ...target, from: fromMs, to: toMs, points })
}
