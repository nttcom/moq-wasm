import type { LiveStats } from '../media/player/locLive'
import type { LogRecord, SpanRecord } from './init'

/// The observability server takes at most this many logs, stats and spans each per report.
const MAX_RECORDS = 100
const FLUSH_INTERVAL_MS = 10_000

export interface TelemetryOptions {
  /// The observability server's origin (`https://observability.example`). Reports go to its
  /// `/api/client-telemetry`. A function is called at each send, for a page that picks the
  /// server from the relay its user chooses.
  url: string | (() => string)
  /// Names the page in each report, at most 128 characters.
  page: string
  /// Returns the fields a log or span is sent with, to mask or drop a value such as a
  /// `track_namespace` that holds a user id. Messages and span names are sent as they are.
  redact?: (fields: Fields) => Fields
  /// Called at each send for the players' stats to send with the session-less records. `redact`
  /// does not apply to them.
  stats?: () => TelemetryStatsSample[]
}

/// One player's `stats()`. A page playing several tracks sets `trackNamespace` (joined with
/// `/`) and `mediaSource`; a page with a single player omits both.
export interface TelemetryStatsSample {
  trackNamespace?: string
  mediaSource?: string
  values: LiveStats
}

type Fields = LogRecord['fields']

type Batch = { logs: LogRecord[]; stats: (TelemetryStatsSample & { timeMs: number })[]; spans: SpanRecord[] }
/// Resolves to the response's HTTP status, or `undefined` when there is none to read.
type Send = (url: string, body: string, beacon: boolean) => Promise<number | undefined>

/// Queues the SDK's logs and spans per session token, with the page's player stats, and posts
/// them to the observability server's `/api/client-telemetry`, which verifies the token and
/// forwards them to the collector.
export class ClientTelemetry {
  readonly clientId = crypto.randomUUID()
  private batches = new Map<string | undefined, Batch>()
  private readonly rejectedTokens = new Set<string>()

  constructor(
    private readonly options: TelemetryOptions,
    private readonly send: Send = sendReport
  ) {}

  readonly log = (record: LogRecord, token: string | undefined): void => {
    keepNewest(this.batch(token).logs, this.redacted(record))
  }

  readonly span = (record: SpanRecord, token: string | undefined): void => {
    keepNewest(this.batch(token).spans, this.redacted(record))
  }

  /// Sends one report per token. `beacon` sends with `navigator.sendBeacon`, which outlives the page.
  flush(beacon = false): void {
    const timeMs = Date.now()
    for (const sample of this.options.stats?.() ?? []) {
      keepNewest(this.batch(undefined).stats, { timeMs, ...sample })
    }
    const batches = this.batches
    this.batches = new Map()
    const url = typeof this.options.url === 'string' ? this.options.url : this.options.url()
    const endpoint = `${url.replace(/\/+$/, '')}/api/client-telemetry`
    for (const [batchToken, { logs, stats, spans }] of batches) {
      if (logs.length === 0 && stats.length === 0 && spans.length === 0) {
        continue
      }
      const token = batchToken !== undefined && this.rejectedTokens.has(batchToken) ? undefined : batchToken
      const body = JSON.stringify({ token, clientId: this.clientId, page: this.options.page, logs, stats, spans })
      void this.send(endpoint, body, beacon).then((status) => {
        if (status === 401 && token !== undefined) {
          this.rejectedTokens.add(token)
        }
      })
    }
  }

  start(): void {
    setInterval(() => this.flush(), FLUSH_INTERVAL_MS)
    addEventListener('pagehide', () => this.flush(true))
  }

  private redacted<T extends { fields: Fields }>(record: T): T {
    const redact = this.options.redact
    return redact ? { ...record, fields: redact(record.fields) } : record
  }

  private batch(token: string | undefined): Batch {
    let batch = this.batches.get(token)
    if (!batch) {
      batch = { logs: [], stats: [], spans: [] }
      this.batches.set(token, batch)
    }
    return batch
  }
}

function keepNewest<T>(records: T[], record: T): void {
  records.push(record)
  if (records.length > MAX_RECORDS) {
    records.shift()
  }
}

/// `text/plain` keeps the POST a CORS simple request, with no preflight.
async function sendReport(url: string, body: string, beacon: boolean): Promise<number | undefined> {
  if (beacon && navigator.sendBeacon(url, body)) {
    return undefined
  }
  try {
    const response = await fetch(url, {
      method: 'POST',
      body,
      keepalive: true,
      headers: { 'content-type': 'text/plain' }
    })
    return response.status
  } catch {
    // Telemetry is best effort: a page whose observability server is down keeps working.
    return undefined
  }
}
