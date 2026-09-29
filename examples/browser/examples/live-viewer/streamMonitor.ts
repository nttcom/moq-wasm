const DEFAULT_KEPT_GROUPS = 5
export const DEFAULT_WINDOW_SECONDS = 10
const AXIS_HEIGHT = 18
const CELL_ROW_HEIGHT = 16
const GRID_LABEL_WIDTH = 78
const CELL_GAP = 1
const GOPS_ACROSS = 3
const DEFAULT_GROUP_LENGTH = 60
const DEFAULT_OBJECT_INTERVAL_MS = 1000 / 30
const GROUP_GAP = 9
const TICK_STEPS_MS = [500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000]
const ROW_HEIGHT = 22
const LABEL_WIDTH = 210
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
  /// FETCH streams span groups, so their arrivals are kept per group.
  receivedAtByGroup: Map<bigint, Map<bigint, number>>
  /// Capture-time span of the objects seen, for the media cadence: arrival
  /// times are bursty and stretch over stalls, capture times are not.
  captureSpanMicros?: { first: number; last: number }
}

/// Live playback follows a subscribe stream, review playback a FETCH stream;
/// each keeps its own playhead so a rewind does not move the live one.
export type Playhead = {
  kind: StreamKind
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
  private readonly playheads = new Map<StreamKind, Playhead>()

  setPlayhead(playhead: Playhead): void {
    this.playheads.set(playhead.kind, playhead)
  }

  clearPlayhead(kind: StreamKind): void {
    this.playheads.delete(kind)
  }

  currentPlayheads(): Playhead[] {
    return [...this.playheads.values()]
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
        receivedAtByObjectId: new Map(),
        receivedAtByGroup: new Map()
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
    now = Date.now(),
    captureMicros?: number
  ): void {
    const record = this.opened(trackAlias, groupId, now)
    if (endsGroup) {
      record.finishedAt = now
      return
    }
    record.bytes += payloadLength
    record.objects += 1
    record.receivedAtByObjectId.set(objectId, now)
    if (captureMicros !== undefined) {
      record.captureSpanMicros = {
        first: Math.min(record.captureSpanMicros?.first ?? captureMicros, captureMicros),
        last: Math.max(record.captureSpanMicros?.last ?? captureMicros, captureMicros)
      }
    }
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
        receivedAtByObjectId: new Map(),
        receivedAtByGroup: new Map()
      }
      this.records.set(key, record)
    }
    record.lastGroupId = groupId
    record.bytes += payloadLength
    record.objects += 1
    let ofGroup = record.receivedAtByGroup.get(groupId)
    if (!ofGroup) {
      ofGroup = new Map()
      record.receivedAtByGroup.set(groupId, ofGroup)
    }
    ofGroup.set(objectId, now)
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
    this.playheads.clear()
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

export function summarizeStreams(records: StreamRecord[], playheads: Playhead[] = [], now = Date.now()): string {
  const open = records.filter((record) => record.finishedAt === undefined).length
  const parts = [`${open} open · ${records.length - open} finished`]
  for (const playhead of playheads) {
    const receivedAt = playheadReceivedAt(records, playhead)
    if (receivedAt === undefined) {
      continue
    }
    const age = ((now - receivedAt) / 1000).toFixed(2)
    const verb = playhead.kind === 'fetch' ? 'reviewing' : 'playing'
    parts.push(`${verb} ${shortGroupId(playhead.groupId)} #${playhead.objectId} received ${age}s ago`)
  }
  return parts.join(' · ')
}

/// The payload bytes of the streams opened within the window, as a rate over it.
export function streamKbps(records: StreamRecord[], windowSeconds: number, now = Date.now()): number {
  const since = now - windowSeconds * 1000
  const bytes = records.filter((record) => record.openedAt >= since).reduce((sum, record) => sum + record.bytes, 0)
  return (bytes * 8) / 1000 / windowSeconds
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

/// A subscribe stream is one group; a FETCH stream (alias = request id) covers
/// every group of its range.
export function playheadRecord(records: StreamRecord[], playhead?: Playhead): StreamRecord | undefined {
  if (!playhead) {
    return undefined
  }
  return records.find(
    (record) =>
      record.kind === playhead.kind &&
      record.trackAlias === playhead.trackAlias &&
      (record.kind === 'fetch'
        ? record.groupId <= playhead.groupId && playhead.groupId <= record.lastGroupId
        : record.groupId === playhead.groupId)
  )
}

export function playheadReceivedAt(records: StreamRecord[], playhead?: Playhead): number | undefined {
  const record = playheadRecord(records, playhead)
  if (!record || !playhead) {
    return undefined
  }
  return record.kind === 'fetch'
    ? record.receivedAtByGroup.get(playhead.groupId)?.get(playhead.objectId)
    : record.receivedAtByObjectId.get(playhead.objectId)
}

export function renderStreamMonitor(
  svg: SVGSVGElement,
  records: StreamRecord[],
  slotsPerTrack: number,
  windowSeconds = DEFAULT_WINDOW_SECONDS,
  playheads: Playhead[] = [],
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
  const playingRecords = playheads.map((playhead) => playheadRecord(records, playhead))
  for (const playhead of playheads) {
    const playheadAt = playheadReceivedAt(records, playhead)
    if (playheadAt === undefined) {
      continue
    }
    const rowsOfKind = slots
      .map((slot, row) => ({
        row,
        kind: slot.record?.kind ?? (slot.track.startsWith('fetch ') ? 'fetch' : 'subscribe')
      }))
      .filter((slot) => slot.kind === playhead.kind)
      .map((slot) => slot.row)
    const top = TOP_MARGIN + (rowsOfKind.length ? Math.min(...rowsOfKind) : 0) * ROW_HEIGHT
    const bottom = TOP_MARGIN + (rowsOfKind.length ? Math.max(...rowsOfKind) + 1 : 1) * ROW_HEIGHT
    const px = x(playheadAt)
    const kindClass = playhead.kind === 'fetch' ? ' stream-playhead-review' : ''
    parts.push(`<line x1="${px}" x2="${px}" y1="${top}" y2="${bottom}" class="stream-playhead${kindClass}" />`)
    parts.push(`<text x="${px}" y="${top - 1}" text-anchor="middle" class="stream-playhead-label${kindClass}">▼</text>`)
  }
  slots.forEach(({ track, record }, row) => {
    const y = TOP_MARGIN + row * ROW_HEIGHT
    const playing = record !== undefined && playingRecords.includes(record)
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

/// Subscribe track aliases and FETCH request ids are separate number spaces
/// that can collide, so a row names its kind. `cadenceAlias` names the live
/// subscription whose object spacing gives the interval; a FETCH row needs it
/// because a FETCH arrives in a burst.
export type DeliveryRow = { label: string; kind: StreamKind; trackAlias?: bigint; cadenceAlias?: bigint }

/// Cells from the playing object rightwards, sized so that `GOPS_ACROSS`
/// groups of the track's usual length fill the width: filled once the object
/// has arrived, hollow until then, so the hollow cells show where the coming
/// frames will land.
export function renderDeliveryGrid(
  svg: SVGSVGElement,
  records: StreamRecord[],
  rows: DeliveryRow[],
  groupsToShow: number,
  from?: Playhead,
  bufferMs = 0
): void {
  const measured = Math.round(svg.getBoundingClientRect().width)
  if (measured === 0) {
    return
  }
  const width = Math.max(measured, 320)
  const height = rows.length * CELL_ROW_HEIGHT + 2 * CELL_GAP
  svg.setAttribute('viewBox', `0 0 ${width} ${height}`)
  svg.setAttribute('height', `${height}`)
  const plotLeft = GRID_LABEL_WIDTH
  const plotRight = width - 4

  const parts: string[] = []
  rows.forEach((row, index) => {
    const y = CELL_GAP + index * CELL_ROW_HEIGHT
    const ofTrack = gridGroups(records, row.kind, row.trackAlias)
    const groupLength = expectedGroupLength(ofTrack) || DEFAULT_GROUP_LENGTH
    const cellSize = Math.max(
      2,
      Math.floor((plotRight - plotLeft - (GOPS_ACROSS - 1) * GROUP_GAP) / (GOPS_ACROSS * groupLength))
    )
    const shown = ofTrack.filter((group) => from === undefined || group.groupId >= from.groupId).slice(-groupsToShow)
    const cadence = row.cadenceAlias === undefined ? ofTrack : gridGroups(records, 'subscribe', row.cadenceAlias)
    const intervalMs = objectIntervalMs(cadence)
    const bufferCells = bufferMs > 0 ? Math.round(bufferMs / intervalMs) : 0
    parts.push(
      `<text x="${GRID_LABEL_WIDTH - 6}" y="${y + CELL_ROW_HEIGHT / 2 + 4}" text-anchor="end" class="stream-label"><title>${escapeXml(`${row.label}: ${ofTrack.length} groups, ${groupLength} objects per group, ${intervalMs.toFixed(1)} ms per object, cell ${cellSize} px`)}</title>${escapeXml(row.label)}</text>`
    )
    if (bufferCells > 0) {
      const bufferWidth = Math.min(
        plotRight - plotLeft,
        bufferCells * cellSize + Math.floor(bufferCells / groupLength) * GROUP_GAP
      )
      parts.push(
        `<rect x="${plotLeft - 1}" y="${y + 1}" width="${bufferWidth + 1}" height="${CELL_ROW_HEIGHT - 2}" rx="2" class="delivery-buffer"><title>${escapeXml(`${row.label}: ${bufferMs} ms playout buffer ≈ ${bufferCells} objects`)}</title></rect>`
      )
    }
    let x = plotLeft
    let groupIndex = 0
    const cellsOf = (group?: GridGroup) =>
      group === undefined
        ? groupLength
        : group.finished
          ? receivedCellCount(group)
          : Math.max(receivedCellCount(group), groupLength)
    while (x + cellSize <= plotRight) {
      const record = shown[groupIndex]
      if (groupIndex > 0) {
        const dividerX = x - GROUP_GAP / 2
        parts.push(
          `<line x1="${dividerX}" x2="${dividerX}" y1="${y + 1}" y2="${y + CELL_ROW_HEIGHT - 1}" class="delivery-divider" />`
        )
      }
      const firstObjectId =
        record !== undefined &&
        from !== undefined &&
        row.trackAlias === from.trackAlias &&
        record.groupId === from.groupId
          ? Number(from.objectId)
          : 0
      const cells = cellsOf(record)
      for (let objectId = firstObjectId; objectId < cells && x + cellSize <= plotRight; objectId++) {
        const received = record?.receivedAt.has(BigInt(objectId)) ?? false
        const title = record
          ? `${row.label} group ${record.groupId} object ${objectId}: ${received ? 'received' : 'missing'}`
          : `${row.label}: not yet received`
        parts.push(
          `<rect x="${x}" y="${y + 3}" width="${Math.max(1, cellSize - CELL_GAP)}" height="${CELL_ROW_HEIGHT - 6}" class="delivery-cell ${received ? 'delivery-cell-received' : 'delivery-cell-missing'}"><title>${escapeXml(title)}</title></rect>`
        )
        x += cellSize
      }
      x += GROUP_GAP
      groupIndex += 1
    }
  })
  svg.innerHTML = parts.join('')
}

type GridGroup = {
  groupId: bigint
  receivedAt: Map<bigint, number>
  finished: boolean
  captureSpanMicros?: { first: number; last: number }
}

/// One grid group per subscribe stream of the alias, or per group inside the
/// FETCH stream whose request id is the alias; a fetched group is complete
/// once a later group of the same FETCH has started.
function gridGroups(records: StreamRecord[], kind: StreamKind, trackAlias?: bigint): GridGroup[] {
  const groups: GridGroup[] = []
  for (const record of records) {
    if (record.kind !== kind || record.trackAlias !== trackAlias) {
      continue
    }
    if (record.kind === 'subscribe') {
      groups.push({
        groupId: record.groupId,
        receivedAt: record.receivedAtByObjectId,
        finished: record.finishedAt !== undefined,
        captureSpanMicros: record.captureSpanMicros
      })
      continue
    }
    for (const [groupId, receivedAt] of record.receivedAtByGroup) {
      groups.push({ groupId, receivedAt, finished: record.finishedAt !== undefined || groupId < record.lastGroupId })
    }
  }
  return groups.sort((a, b) => Number(a.groupId - b.groupId))
}

function receivedCellCount(group: GridGroup): number {
  let highest = -1
  for (const objectId of group.receivedAt.keys()) {
    highest = Math.max(highest, Number(objectId))
  }
  return highest + 1
}

function expectedGroupLength(groups: GridGroup[]): number {
  const lastFinished = groups.filter((group) => group.finished).at(-1)
  return lastFinished ? receivedCellCount(lastFinished) : 0
}

/// Media-time spacing of the track's objects, from the capture timestamps of
/// the last finished group, falling back to arrival spacing without them and
/// to 30 fps until a group has finished.
function objectIntervalMs(groups: GridGroup[]): number {
  const lastFinished = groups.filter((group) => group.finished).at(-1)
  const count = lastFinished ? receivedCellCount(lastFinished) : 0
  if (!lastFinished || count < 2) {
    return DEFAULT_OBJECT_INTERVAL_MS
  }
  const span = lastFinished.captureSpanMicros
  if (span && span.last > span.first) {
    return Math.max(1, (span.last - span.first) / 1000 / (count - 1))
  }
  const arrivals = [...lastFinished.receivedAt.values()]
  return Math.max(1, (Math.max(...arrivals) - Math.min(...arrivals)) / count)
}
