const DEFAULT_KEPT_GROUPS = 5
export const DEFAULT_WINDOW_SECONDS = 10
const AXIS_HEIGHT = 18
const CELL_ROW_HEIGHT = 16
const CELL_GAP = 1
const GROUP_GAP = 6
const TICK_STEPS_MS = [500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000]
const ROW_HEIGHT = 22
const LABEL_WIDTH = 168
const TOP_MARGIN = 14
const PALETTE = ['#0f766e', '#e07a5f', '#3d5a80', '#b08968', '#6a4c93', '#2a9d8f']

export type StreamKind = 'subscribe' | 'fetch'

export type StreamRecord = {
  kind: StreamKind
  trackAlias: bigint
  track: string
  groupId: bigint
  lastGroupId: bigint
  openedAt: number
  finishedAt?: number
  bytes: number
  objects: number
  receivedAtByObjectId: Map<bigint, number>
}

export type Playhead = {
  trackAlias: bigint
  groupId: bigint
  objectId: bigint
}

/// Subgroup streams the viewer has received, keyed by track alias and group;
/// a stream counts as finished when its end-of-group status object arrives,
/// and the transport closing it without one is not observable from JS. Only
/// the newest `keptGroups` groups of each track are kept.
export class StreamMonitor {
  private readonly records = new Map<string, StreamRecord>()
  private readonly labels = new Map<bigint, string>()
  private keptGroups = DEFAULT_KEPT_GROUPS
  private playhead?: Playhead

  setPlayhead(playhead: Playhead): void {
    this.playhead = playhead
  }

  currentPlayhead(): Playhead | undefined {
    return this.playhead
  }

  setKeptGroups(count: number): void {
    this.keptGroups = Math.max(1, Math.floor(count))
  }

  slotsPerTrack(): number {
    return this.keptGroups
  }

  label(trackAlias: bigint, track: string): void {
    this.labels.set(trackAlias, track)
  }

  opened(trackAlias: bigint, groupId: bigint, now = Date.now()): StreamRecord {
    const key = `${trackAlias}:${groupId}`
    let record = this.records.get(key)
    if (!record) {
      record = {
        kind: 'subscribe',
        trackAlias,
        track: this.labels.get(trackAlias) ?? `alias ${trackAlias}`,
        groupId,
        lastGroupId: groupId,
        openedAt: now,
        bytes: 0,
        objects: 0,
        receivedAtByObjectId: new Map()
      }
      this.records.set(key, record)
    }
    return record
  }

  object(
    trackAlias: bigint,
    groupId: bigint,
    objectId: bigint,
    payloadLength: number,
    endsGroup: boolean,
    now = Date.now()
  ): void {
    const record = this.opened(trackAlias, groupId, now)
    if (endsGroup) {
      record.finishedAt = now
      return
    }
    record.bytes += payloadLength
    record.objects += 1
    record.receivedAtByObjectId.set(objectId, now)
  }

  /// One FETCH response is one stream however many groups it spans; the
  /// request id stands in for the group id in the row key.
  fetchObject(
    requestId: bigint,
    track: string,
    groupId: bigint,
    objectId: bigint,
    payloadLength: number,
    now = Date.now()
  ): void {
    const key = `fetch:${requestId}`
    let record = this.records.get(key)
    if (!record) {
      record = {
        kind: 'fetch',
        trackAlias: requestId,
        track: `fetch ${track}`,
        groupId: groupId,
        lastGroupId: groupId,
        openedAt: now,
        bytes: 0,
        objects: 0,
        receivedAtByObjectId: new Map()
      }
      this.records.set(key, record)
    }
    record.lastGroupId = groupId
    record.bytes += payloadLength
    record.objects += 1
    record.receivedAtByObjectId.set(objectId, now)
  }

  fetchFinished(requestId: bigint, now = Date.now()): void {
    const record = this.records.get(`fetch:${requestId}`)
    if (record && record.finishedAt === undefined) {
      record.finishedAt = now
    }
  }

  reset(): void {
    this.records.clear()
    this.labels.clear()
    this.playhead = undefined
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
    return [...this.records.values()].sort(compareRecords)
  }
}

export function renderIdleStreamMonitor(svg: SVGSVGElement): void {
  svg.setAttribute('height', `${TOP_MARGIN * 2 + ROW_HEIGHT}`)
  svg.removeAttribute('viewBox')
  svg.innerHTML = ''
}

export function summarizeStreams(records: StreamRecord[], playhead?: Playhead, now = Date.now()): string {
  const open = records.filter((record) => record.finishedAt === undefined).length
  const counts = `${open} open · ${records.length - open} finished`
  const receivedAt = playheadReceivedAt(records, playhead)
  if (!playhead || receivedAt === undefined) {
    return counts
  }
  const age = ((now - receivedAt) / 1000).toFixed(2)
  return `${counts} · playing ${shortGroupId(playhead.groupId)} #${playhead.objectId} received ${age}s ago`
}

/// FETCH streams come first, then subscriptions by track name.
function compareRecords(a: StreamRecord, b: StreamRecord): number {
  if (a.kind !== b.kind) {
    return a.kind === 'fetch' ? -1 : 1
  }
  return a.track === b.track ? Number(a.groupId - b.groupId) : a.track.localeCompare(b.track)
}

function slotKey(record: StreamRecord): bigint {
  return record.kind === 'fetch' ? record.trackAlias : record.groupId
}

type Slot = { track: string; record?: StreamRecord }

/// Every track owns a fixed block of rows and a group always sits in the row
/// `groupId mod slots`, so the group that replaces an evicted one lands in the
/// same row and nothing below it moves.
export function layoutSlots(records: StreamRecord[], slotsPerTrack: number): Slot[] {
  const ordered = [...records].sort(compareRecords)
  const tracks = [...new Set(ordered.map((record) => record.track))]
  const slots: Slot[] = []
  for (const track of tracks) {
    const ofTrack = ordered.filter((record) => record.track === track)
    const count = Math.min(slotsPerTrack, ofTrack.length)
    for (let slot = 0; slot < count; slot++) {
      const record = ofTrack
        .filter((candidate) => Number(slotKey(candidate) % BigInt(count)) === slot)
        .sort((a, b) => b.openedAt - a.openedAt)[0]
      slots.push({ track, record })
    }
  }
  return slots
}

export function playheadReceivedAt(records: StreamRecord[], playhead?: Playhead): number | undefined {
  if (!playhead) {
    return undefined
  }
  return records
    .find((record) => record.trackAlias === playhead.trackAlias && record.groupId === playhead.groupId)
    ?.receivedAtByObjectId.get(playhead.objectId)
}

export function renderStreamMonitor(
  svg: SVGSVGElement,
  records: StreamRecord[],
  slotsPerTrack: number,
  windowSeconds = DEFAULT_WINDOW_SECONDS,
  playhead?: Playhead,
  now = Date.now()
): void {
  const slots = layoutSlots(records, slotsPerTrack)
  const tracks = [...new Set(slots.map((slot) => slot.track))]
  const measured = Math.round(svg.getBoundingClientRect().width)
  if (measured === 0) {
    return
  }
  const width = Math.max(measured, 320)
  const height = TOP_MARGIN * 2 + Math.max(slots.length, 1) * ROW_HEIGHT + AXIS_HEIGHT
  svg.setAttribute('viewBox', `0 0 ${width} ${height}`)
  svg.setAttribute('height', `${height}`)
  const plotLeft = LABEL_WIDTH
  const plotRight = width - 8
  const plotWidth = plotRight - plotLeft
  const windowMs = Math.max(1, windowSeconds) * 1000
  const oldest = now - windowMs
  const x = (at: number) => plotLeft + Math.max(0, plotWidth - ((now - at) * plotWidth) / windowMs)

  const parts: string[] = []
  const axisY = TOP_MARGIN + Math.max(slots.length, 1) * ROW_HEIGHT
  for (const tick of timeTicks(oldest, now)) {
    const tx = x(tick)
    parts.push(`<line x1="${tx}" x2="${tx}" y1="${TOP_MARGIN}" y2="${axisY}" class="stream-tick" />`)
    parts.push(`<text x="${tx}" y="${axisY + 13}" text-anchor="middle" class="stream-axis">${clockLabel(tick)}</text>`)
  }
  const playheadAt = playheadReceivedAt(records, playhead)
  if (playheadAt !== undefined) {
    const px = x(playheadAt)
    parts.push(`<line x1="${px}" x2="${px}" y1="${TOP_MARGIN}" y2="${axisY}" class="stream-playhead" />`)
    parts.push(`<text x="${px}" y="${TOP_MARGIN - 1}" text-anchor="middle" class="stream-playhead-label">▼</text>`)
  }
  slots.forEach(({ track, record }, row) => {
    const y = TOP_MARGIN + row * ROW_HEIGHT
    const playing =
      playhead !== undefined && record?.trackAlias === playhead.trackAlias && record.groupId === playhead.groupId
    parts.push(
      `<line x1="${plotLeft}" x2="${plotRight}" y1="${y + ROW_HEIGHT - 2}" y2="${y + ROW_HEIGHT - 2}" class="stream-baseline" />`
    )
    if (!record) {
      parts.push(
        `<text x="${LABEL_WIDTH - 8}" y="${y + ROW_HEIGHT / 2 + 4}" text-anchor="end" class="stream-label">${escapeXml(track)}</text>`
      )
      return
    }
    const color = PALETTE[tracks.indexOf(track) % PALETTE.length]
    const open = record.finishedAt === undefined
    const start = x(record.openedAt)
    const end = open ? plotRight : x(record.finishedAt as number)
    const groups =
      record.lastGroupId === record.groupId
        ? shortGroupId(record.groupId)
        : `${shortGroupId(record.groupId)}–${shortGroupId(record.lastGroupId)}`
    const label = `${record.track} ${groups}`
    const title = `${record.track} group ${record.groupId}${record.lastGroupId === record.groupId ? '' : `–${record.lastGroupId}`}: opened ${clockLabel(record.openedAt)}, ${record.objects} objects, ${(record.bytes / 1024).toFixed(1)} KB, ${open ? 'open' : `finished ${clockLabel(record.finishedAt as number)}`}`
    parts.push(
      `<text x="${LABEL_WIDTH - 8}" y="${y + ROW_HEIGHT / 2 + 4}" text-anchor="end" class="stream-label"><title>${escapeXml(title)}</title>${escapeXml(label)}</text>`
    )
    parts.push(
      `<rect x="${start}" y="${y + 3}" width="${Math.max(end - start, 2)}" height="${ROW_HEIGHT - 8}" rx="3" fill="${color}" class="stream-bar${open ? ' stream-bar-open' : ''}${playing ? ' stream-bar-playing' : ''}${record.kind === 'fetch' ? ' stream-bar-fetch' : ''}"><title>${escapeXml(title)}</title></rect>`
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

export type DeliveryRow = { label: string; trackAlias?: bigint }

/// One cell per object of the newest `groupsToShow` groups of each track:
/// filled when it has arrived, hollow when a later object of the same group
/// has arrived but this one has not (or, for a finished group, never did).
export function renderDeliveryGrid(
  svg: SVGSVGElement,
  records: StreamRecord[],
  rows: DeliveryRow[],
  groupsToShow: number
): void {
  const measured = Math.round(svg.getBoundingClientRect().width)
  if (measured === 0) {
    return
  }
  const width = Math.max(measured, 320)
  const height = TOP_MARGIN + rows.length * CELL_ROW_HEIGHT + TOP_MARGIN
  svg.setAttribute('viewBox', `0 0 ${width} ${height}`)
  svg.setAttribute('height', `${height}`)
  const plotLeft = LABEL_WIDTH
  const plotWidth = width - 8 - plotLeft

  const rowGroups = rows.map((row) =>
    records
      .filter((record) => record.kind === 'subscribe' && record.trackAlias === row.trackAlias)
      .sort((a, b) => Number(a.groupId - b.groupId))
      .slice(-groupsToShow)
      .map((record) => ({ record, cells: cellCount(record) }))
  )
  const widestRow = Math.max(
    1,
    ...rowGroups.map(
      (groups) => groups.reduce((sum, group) => sum + group.cells, 0) + Math.max(0, groups.length - 1) * 2
    )
  )
  const cell = Math.max(2, Math.min(10, Math.floor(plotWidth / widestRow)))

  const parts: string[] = []
  rows.forEach((row, index) => {
    const y = TOP_MARGIN + index * CELL_ROW_HEIGHT
    parts.push(
      `<text x="${LABEL_WIDTH - 8}" y="${y + CELL_ROW_HEIGHT / 2 + 4}" text-anchor="end" class="stream-label">${escapeXml(row.label)}</text>`
    )
    let x = plotLeft
    for (const { record, cells } of rowGroups[index]) {
      for (let objectId = 0; objectId < cells; objectId++) {
        const received = record.receivedAtByObjectId.has(BigInt(objectId))
        parts.push(
          `<rect x="${x}" y="${y + 3}" width="${cell - CELL_GAP}" height="${CELL_ROW_HEIGHT - 6}" class="delivery-cell ${received ? 'delivery-cell-received' : 'delivery-cell-missing'}"><title>${escapeXml(`${record.track} group ${record.groupId} object ${objectId}: ${received ? 'received' : 'missing'}`)}</title></rect>`
        )
        x += cell
      }
      x += GROUP_GAP
    }
  })
  svg.innerHTML = parts.join('')
}

function cellCount(record: StreamRecord): number {
  let highest = -1
  for (const objectId of record.receivedAtByObjectId.keys()) {
    highest = Math.max(highest, Number(objectId))
  }
  return highest + 1
}
