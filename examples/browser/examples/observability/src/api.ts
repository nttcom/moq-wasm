import { bearerToken } from '../../../utils/access'

/** 64-bit ids arrive as strings: JavaScript numbers hold 53 bits. */
export type Id = string

/** A client session of the app the page's token names. */
export interface SessionStats {
  session_id: Id
  peer: 'client'
  app_id: string
  remote_address: string | null
  local_ip: string | null
  dialed_relay_id: string | null
  rtt_us: number
  current_mtu: number
  sent_bytes: number
  sent_packets: number
  lost_packets: number
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

/**
 * A session with another relay that one of the app's tracks or subscriptions names. The server
 * leaves out its counters, which every app's traffic adds to.
 */
export interface RelaySession {
  session_id: Id
  peer: 'relay'
  remote_address: string | null
  local_ip: string | null
  dialed_relay_id: string | null
}

export interface TrackStats {
  namespace: string
  name: string
  publisher_session_id: Id
  bytes_received: number
  max_arrival_gap_since_last_snapshot_us: number
}

export interface SubscriptionStats {
  namespace: string
  name: string
  publisher_session_id: Id
  subscriber_session_id: Id
  request_id: Id
  bytes_sent: number
  streams_reset: number
  lag_behind_newest_received_us: number
}

/** A relay's snapshot as the app the page's token names may see it. */
export interface RelaySnapshot {
  relay_id: string
  timestamp_ms: number
  sessions: (SessionStats | RelaySession)[]
  tracks: TrackStats[]
  subscriptions: SubscriptionStats[]
}

export type SeriesTarget =
  /** The app's client sessions on the relay, summed. */
  | { target: 'relay' }
  | { target: 'session'; session_id: Id }
  | { target: 'track'; publisher_session_id: Id; namespace: string; name: string }
  | { target: 'subscription'; subscriber_session_id: Id; request_id: Id }

export interface Series {
  t: number[]
  series: Record<string, (number | null)[]>
}

const LOCAL_HOSTS = ['localhost', '127.0.0.1']
const API_BASE = LOCAL_HOSTS.includes(window.location.hostname)
  ? 'http://localhost:8095'
  : 'https://observability.moqt.research.skyway.io'

async function getJson<T>(path: string, params: Record<string, string | number>): Promise<T> {
  const query = new URLSearchParams(Object.entries(params).map(([key, value]) => [key, String(value)]))
  const token = bearerToken()
  const response = await fetch(`${API_BASE}${path}?${query}`, {
    headers: token ? { Authorization: `Bearer ${token}` } : {}
  })
  if (!response.ok) {
    throw new Error(`${path} answered ${response.status}: ${await response.text()}`)
  }
  return (await response.json()) as T
}

export function fetchSnapshots(): Promise<RelaySnapshot[]> {
  return getJson('/api/snapshots', {})
}

export interface PastSnapshots {
  snapshots: RelaySnapshot[]
  previous: RelaySnapshot[]
}

/** `previousMs` is at most 10 s before `atMs`. */
export function fetchPastSnapshots(atMs: number, previousMs: number): Promise<PastSnapshots> {
  return getJson('/api/snapshots', { at: atMs, previous: previousMs })
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

export interface NamespaceSummary {
  namespace: string
  tracks: string[]
  relays: string[]
  first_seen_ms: number
  last_seen_ms: number
  subscriptions: number
}

export interface PublishedNamespaces {
  namespaces: NamespaceSummary[]
  subscriptions: number
  clients: number
}

export function fetchNamespaces(fromMs: number, toMs: number): Promise<PublishedNamespaces> {
  return getJson('/api/namespaces', { from: fromMs, to: toMs })
}
