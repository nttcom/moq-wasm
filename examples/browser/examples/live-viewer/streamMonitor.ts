const DEFAULT_KEPT_GROUPS = 5
export const DEFAULT_WINDOW_SECONDS = 10
const AXIS_HEIGHT = 18
const TICK_STEPS_MS = [500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000]
const ROW_HEIGHT = 22
const LABEL_WIDTH = 132
const TOP_MARGIN = 6
const PALETTE = ['#0f766e', '#e07a5f', '#3d5a80', '#b08968', '#6a4c93', '#2a9d8f']

export type StreamRecord = {
  trackAlias: bigint
  track: string
  groupId: bigint
  openedAt: number
  finishedAt?: number
  bytes: number
  objects: number
}

/// Subgroup streams the viewer has received, keyed by track alias and group;
/// a stream counts as finished when its end-of-group status object arrives,
/// and the transport closing it without one is not observable from JS. Only
/// the newest `keptGroups` groups of each track are kept.
export class StreamMonitor {
  private readonly records = new Map<string, StreamRecord>()
  private readonly labels = new Map<bigint, string>()
  private keptGroups = DEFAULT_KEPT_GROUPS

  setKeptGroups(count: number): void {
    this.keptGroups = Math.max(1, Math.floor(count))
  }

  label(trackAlias: bigint, track: string): void {
    this.labels.set(trackAlias, track)
  }

  opened(trackAlias: bigint, groupId: bigint, now = Date.now()): StreamRecord {
    const key = `${trackAlias}:${groupId}`
    let record = this.records.get(key)
    if (!record) {
      record = {
        trackAlias,
        track: this.labels.get(trackAlias) ?? `alias ${trackAlias}`,
        groupId,
        openedAt: now,
        bytes: 0,
        objects: 0
      }
      this.records.set(key, record)
    }
    return record
  }

  object(trackAlias: bigint, groupId: bigint, payloadLength: number, endsGroup: boolean, now = Date.now()): void {
    const record = this.opened(trackAlias, groupId, now)
    if (endsGroup) {
      record.finishedAt = now
      return
    }
    record.bytes += payloadLength
    record.objects += 1
  }

  reset(): void {
    this.records.clear()
    this.labels.clear()
  }

  snapshot(): StreamRecord[] {
    const newestFirst = [...this.records.entries()].sort(([, a], [, b]) => b.openedAt - a.openedAt)
    const seen = new Map<string, number>()
    for (const [key, record] of newestFirst) {
      const kept = (seen.get(record.track) ?? 0) + 1
      seen.set(record.track, kept)
      if (kept > this.keptGroups) {
        this.records.delete(key)
      }
    }
    return [...this.records.values()].sort((a, b) =>
      a.track === b.track ? Number(a.groupId - b.groupId) : a.track.localeCompare(b.track)
    )
  }
}

export function summarizeStreams(records: StreamRecord[]): string {
  const open = records.filter((record) => record.finishedAt === undefined).length
  return `${open} open · ${records.length - open} finished`
}

export function renderStreamMonitor(
  svg: SVGSVGElement,
  records: StreamRecord[],
  windowSeconds = DEFAULT_WINDOW_SECONDS,
  now = Date.now()
): void {
  const tracks = [...new Set(records.map((record) => record.track))]
  const width = Math.max(Math.round(svg.getBoundingClientRect().width), 320)
  const height = TOP_MARGIN * 2 + Math.max(records.length, 1) * ROW_HEIGHT + AXIS_HEIGHT
  svg.setAttribute('viewBox', `0 0 ${width} ${height}`)
  svg.setAttribute('height', `${height}`)
  const plotLeft = LABEL_WIDTH
  const plotRight = width - 8
  const plotWidth = plotRight - plotLeft
  const windowMs = Math.max(1, windowSeconds) * 1000
  const oldest = now - windowMs
  const x = (at: number) => plotLeft + Math.max(0, plotWidth - ((now - at) * plotWidth) / windowMs)

  const parts: string[] = []
  const axisY = TOP_MARGIN + Math.max(records.length, 1) * ROW_HEIGHT
  for (const tick of timeTicks(oldest, now)) {
    const tx = x(tick)
    parts.push(`<line x1="${tx}" x2="${tx}" y1="${TOP_MARGIN}" y2="${axisY}" class="stream-tick" />`)
    parts.push(`<text x="${tx}" y="${axisY + 13}" text-anchor="middle" class="stream-axis">${clockLabel(tick)}</text>`)
  }
  records.forEach((record, row) => {
    const y = TOP_MARGIN + row * ROW_HEIGHT
    const color = PALETTE[tracks.indexOf(record.track) % PALETTE.length]
    const open = record.finishedAt === undefined
    const start = x(record.openedAt)
    const end = open ? plotRight : x(record.finishedAt as number)
    const label = `${record.track} ${shortGroupId(record.groupId)}`
    const title = `${record.track} group ${record.groupId}: opened ${clockLabel(record.openedAt)}, ${record.objects} objects, ${(record.bytes / 1024).toFixed(1)} KB, ${open ? 'open' : `finished ${clockLabel(record.finishedAt as number)}`}`
    parts.push(
      `<text x="${LABEL_WIDTH - 8}" y="${y + ROW_HEIGHT / 2 + 4}" text-anchor="end" class="stream-label"><title>${escapeXml(title)}</title>${escapeXml(label)}</text>`
    )
    parts.push(
      `<line x1="${plotLeft}" x2="${plotRight}" y1="${y + ROW_HEIGHT - 2}" y2="${y + ROW_HEIGHT - 2}" class="stream-baseline" />`
    )
    parts.push(
      `<rect x="${start}" y="${y + 3}" width="${Math.max(end - start, 2)}" height="${ROW_HEIGHT - 8}" rx="3" fill="${color}" class="stream-bar${open ? ' stream-bar-open' : ''}"><title>${escapeXml(title)}</title></rect>`
    )
  })
  svg.innerHTML = parts.join('')
}

function timeTicks(from: number, to: number): number[] {
  const step =
    TICK_STEPS_MS.find((candidate) => (to - from) / candidate <= 8) ?? TICK_STEPS_MS[TICK_STEPS_MS.length - 1]
  const ticks: number[] = []
  for (let tick = Math.ceil(from / step) * step; tick <= to; tick += step) {
    ticks.push(tick)
  }
  return ticks
}

function clockLabel(at: number): string {
  const date = new Date(at)
  const hms = date.toLocaleTimeString('en-GB', { hour12: false })
  return `${hms}.${String(date.getMilliseconds()).padStart(3, '0').slice(0, 1)}`
}

/// Group ids are time-seeded 16-digit numbers, so only the tail distinguishes
/// neighbouring groups; the full id stays in the tooltip.
function shortGroupId(groupId: bigint): string {
  const digits = groupId.toString()
  return digits.length > 6 ? `…${digits.slice(-6)}` : digits
}

function escapeXml(text: string): string {
  return text.replace(/[<>&"]/g, (char) => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;' })[char] ?? char)
}
