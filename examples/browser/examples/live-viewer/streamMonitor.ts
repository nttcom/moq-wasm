const WINDOW_MS = 20_000
const FINISHED_RETENTION_MS = 5_000
const ROW_HEIGHT = 22
const LABEL_WIDTH = 96
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
/// and the transport closing it without one is not observable from JS.
export class StreamMonitor {
  private readonly records = new Map<string, StreamRecord>()
  private readonly labels = new Map<bigint, string>()

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

  snapshot(now = Date.now()): StreamRecord[] {
    for (const [key, record] of this.records) {
      if (record.finishedAt !== undefined && now - record.finishedAt > FINISHED_RETENTION_MS) {
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
  return `${open} open · ${records.length - open} finished within 5s`
}

export function renderStreamMonitor(svg: SVGSVGElement, records: StreamRecord[], now = Date.now()): void {
  const tracks = [...new Set(records.map((record) => record.track))]
  const width = Math.max(Math.round(svg.getBoundingClientRect().width), 320)
  const height = TOP_MARGIN * 2 + Math.max(tracks.length, 1) * ROW_HEIGHT
  svg.setAttribute('viewBox', `0 0 ${width} ${height}`)
  svg.setAttribute('height', `${height}`)
  const plotWidth = width - LABEL_WIDTH - 8
  const x = (at: number) => LABEL_WIDTH + Math.max(0, plotWidth - ((now - at) * plotWidth) / WINDOW_MS)

  const parts: string[] = []
  tracks.forEach((track, row) => {
    const y = TOP_MARGIN + row * ROW_HEIGHT
    const color = PALETTE[row % PALETTE.length]
    parts.push(
      `<text x="${LABEL_WIDTH - 8}" y="${y + ROW_HEIGHT / 2 + 4}" text-anchor="end" class="stream-label">${escapeXml(track)}</text>`
    )
    parts.push(
      `<line x1="${LABEL_WIDTH}" x2="${width - 8}" y1="${y + ROW_HEIGHT - 2}" y2="${y + ROW_HEIGHT - 2}" class="stream-baseline" />`
    )
    for (const record of records.filter((candidate) => candidate.track === track)) {
      const start = x(record.openedAt)
      const end = record.finishedAt === undefined ? width - 8 : x(record.finishedAt)
      const open = record.finishedAt === undefined
      const title = `${track} group ${record.groupId}: ${record.objects} objects, ${(record.bytes / 1024).toFixed(1)} KB, ${open ? 'open' : 'finished'}`
      parts.push(
        `<rect x="${start}" y="${y + 3}" width="${Math.max(end - start, 2)}" height="${ROW_HEIGHT - 8}" rx="3" fill="${color}" class="stream-bar${open ? ' stream-bar-open' : ''}"><title>${escapeXml(title)}</title></rect>`
      )
    }
  })
  svg.innerHTML = parts.join('')
}

function escapeXml(text: string): string {
  return text.replace(/[<>&"]/g, (char) => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;' })[char] ?? char)
}
