import type { LogRecord, SpanRecord } from 'moq-wasm'
import type { LiveStats } from 'moq-wasm/media'

/// The observability server takes at most this many logs, stats and spans each per report.
const MAX_RECORDS = 100

type ReportLog = Omit<LogRecord, 'level'> & { level: string }
/// One player's stats. A page playing several tracks sets `trackNamespace`
/// (joined with `/`) and `mediaSource`; a single-player page omits both.
export type StatsSample = { trackNamespace?: string; mediaSource?: string; values: LiveStats }
type ReportStats = StatsSample & { timeMs: number }
type Send = (url: string, body: string, beacon: boolean) => void

/// Collects a page's logs, player stats and wasm spans and posts them to the observability
/// server's `/api/client-telemetry`, which forwards them to the collector.
export class ClientTelemetry {
  readonly clientId = crypto.randomUUID()
  private logs: ReportLog[] = []
  private stats: ReportStats[] = []
  private spans: SpanRecord[] = []

  constructor(
    private readonly page: string,
    private readonly endpoint: () => string,
    private readonly send: Send = sendReport
  ) {}

  log(level: string, message: string, fields: LogRecord['fields'] = {}): void {
    this.wasmLog({ timeMs: Date.now(), level, target: this.page, message, fields })
  }

  readonly wasmLog = (record: ReportLog): void => {
    keepNewest(this.logs, record)
  }

  readonly wasmSpan = (record: SpanRecord): void => {
    keepNewest(this.spans, record)
  }

  sample(sample: StatsSample): void {
    keepNewest(this.stats, { timeMs: Date.now(), ...sample })
  }

  /// `beacon` sends with `navigator.sendBeacon`, which outlives the page.
  flush(beacon = false): void {
    if (this.logs.length === 0 && this.stats.length === 0 && this.spans.length === 0) {
      return
    }
    const body = JSON.stringify({
      clientId: this.clientId,
      page: this.page,
      logs: this.logs,
      stats: this.stats,
      spans: this.spans
    })
    this.logs = []
    this.stats = []
    this.spans = []
    this.send(this.endpoint(), body, beacon)
  }
}

function keepNewest<T>(records: T[], record: T): void {
  records.push(record)
  if (records.length > MAX_RECORDS) {
    records.shift()
  }
}

/// `text/plain` keeps the POST a CORS simple request, with no preflight.
function sendReport(url: string, body: string, beacon: boolean): void {
  if (beacon && navigator.sendBeacon(url, body)) {
    return
  }
  // Telemetry is best effort: a page without an observability server keeps working.
  fetch(url, { method: 'POST', body, keepalive: true, headers: { 'content-type': 'text/plain' } }).catch(() => {})
}
