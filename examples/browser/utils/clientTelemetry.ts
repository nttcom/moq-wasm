import type { LogRecord, SpanRecord } from 'moq-wasm'
import type { LiveStats } from 'moq-wasm/media'

/// The observability server takes at most this many logs, stats and spans each per report.
const MAX_RECORDS = 100

type ReportLog = Omit<LogRecord, 'level'> & { level: string }
/// One player's stats. A page playing several tracks sets `trackNamespace`
/// (joined with `/`) and `mediaSource`; a single-player page omits both.
export type StatsSample = { trackNamespace?: string; mediaSource?: string; values: LiveStats }
type ReportStats = StatsSample & { timeMs: number }
type Batch = { logs: ReportLog[]; stats: ReportStats[]; spans: SpanRecord[] }
/// Resolves to the response's HTTP status, or `undefined` when there is none to read.
type Send = (url: string, body: string, beacon: boolean) => Promise<number | undefined>

/// Collects a page's logs, player stats and wasm spans and posts them to the observability
/// server's `/api/client-telemetry`, which forwards them to the collector.
export class ClientTelemetry {
  readonly clientId = crypto.randomUUID()
  private batches = new Map<string | undefined, Batch>()
  private readonly rejectedTokens = new Set<string>()

  constructor(
    private readonly page: string,
    private readonly endpoint: () => string,
    private readonly send: Send = sendReport
  ) {}

  log(level: string, message: string, fields: LogRecord['fields'] = {}): void {
    this.wasmLog({ timeMs: Date.now(), level, target: this.page, message, fields }, undefined)
  }

  readonly wasmLog = (record: ReportLog, token: string | undefined): void => {
    keepNewest(this.batch(token).logs, record)
  }

  readonly wasmSpan = (record: SpanRecord, token: string | undefined): void => {
    keepNewest(this.batch(token).spans, record)
  }

  sample(sample: StatsSample): void {
    keepNewest(this.batch(undefined).stats, { timeMs: Date.now(), ...sample })
  }

  /// Sends one report per token. `beacon` sends with `navigator.sendBeacon`, which outlives the page.
  flush(beacon = false): void {
    const batches = this.batches
    this.batches = new Map()
    for (const [batchToken, { logs, stats, spans }] of batches) {
      if (logs.length === 0 && stats.length === 0 && spans.length === 0) {
        continue
      }
      const token = batchToken !== undefined && this.rejectedTokens.has(batchToken) ? undefined : batchToken
      const body = JSON.stringify({ token, clientId: this.clientId, page: this.page, logs, stats, spans })
      void this.send(this.endpoint(), body, beacon).then((status) => {
        if (status === 401 && token !== undefined) {
          this.rejectedTokens.add(token)
        }
      })
    }
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
    // Telemetry is best effort: a page without an observability server keeps working.
    return undefined
  }
}
