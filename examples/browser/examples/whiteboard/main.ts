import { type Announcement, MoqSession, type MoqNamespaceWatch, type MoqSubscription, type TrackWriter } from 'moq-wasm'
import { connectOptions, withNamespaceRoot } from '../../utils/access'
import { pageRoomId } from '../../utils/botRoom'
import { setUpRelayUrlControls } from '../../utils/relayPicker'
import { randomName } from '../../utils/randomName'
import { element, getErrorMessage } from '../../utils/page'
import { startPageTelemetry } from '../../utils/pageTelemetry'
import { Board, type BoardOp, type Stroke, decodeOp, encodeOp, strokeId } from './board'

const TRACK = 'board'
/// A new group, starting with a snapshot, after this many of this page's own operations.
const OPS_PER_GROUP = 100
/// Long enough that the publisher answers the FETCH of a late joiner while the board sits idle.
const RETENTION_MS = 60 * 60 * 1000
const POINTS_FLUSH_MS = 40
/// The board's height over its width; points are stored in [0, 1] of each side.
const ASPECT = 9 / 16
const ERASER_RADIUS_PX = 10

type Tool = 'pen' | 'eraser'

type Joined = {
  session: MoqSession
  me: string
  announcement: Announcement
  writer: TrackWriter
  watch: MoqNamespaceWatch
  peers: Map<string, MoqSubscription>
  /// Writes run one after another so objects keep their order.
  writing: Promise<unknown>
  opsInGroup: number
}

type Drawing = { id: string; points: number[]; sent: number }

const urlInput = element<HTMLInputElement>('url')
const joinButton = element<HTMLButtonElement>('joinBtn')
const leaveButton = element<HTMLButtonElement>('leaveBtn')
const clearButton = element<HTMLButtonElement>('clearBtn')
const canvas = element<HTMLCanvasElement>('board')
const colorInput = element<HTMLInputElement>('color')
const widthInput = element<HTMLInputElement>('width')
const participantList = element<HTMLUListElement>('participants')
const strokeCount = element<HTMLElement>('strokeCount')

startPageTelemetry('whiteboard', () => urlInput.value.trim())
setUpRelayUrlControls(urlInput, element('urlPresets'))
const ROOM = pageRoomId()
const ROOM_NAMESPACE = withNamespaceRoot(['anon', 'whiteboard', ROOM])
element('room').textContent = ROOM

let joined: Joined | undefined
let board = new Board()
let tool: Tool = 'pen'
let drawing: Drawing | undefined
let redrawPending = false

joinButton.addEventListener('click', () => void join())
leaveButton.addEventListener('click', () => void leave('退出しました'))
clearButton.addEventListener('click', () => {
  if (joined) {
    publish(joined, { kind: 'clear', at: board.nextStamp(joined.me) })
  }
})
for (const button of document.querySelectorAll<HTMLButtonElement>('[data-tool]')) {
  button.addEventListener('click', () => selectTool(button.dataset.tool as Tool))
}
canvas.addEventListener('pointerdown', onPointerDown)
canvas.addEventListener('pointermove', onPointerMove)
canvas.addEventListener('pointerup', endStroke)
canvas.addEventListener('pointercancel', endStroke)
new ResizeObserver(requestRedraw).observe(canvas)
setInterval(flushPoints, POINTS_FLUSH_MS)
setEnabled(false)
renderParticipants()

function setStatus(text: string): void {
  element('status').textContent = text
}

function setEnabled(enabled: boolean): void {
  joinButton.disabled = enabled
  leaveButton.disabled = !enabled
  clearButton.disabled = !enabled
  canvas.classList.toggle('disabled', !enabled)
}

function selectTool(selected: Tool): void {
  tool = selected
  for (const button of document.querySelectorAll<HTMLButtonElement>('[data-tool]')) {
    button.setAttribute('aria-pressed', String(button.dataset.tool === selected))
  }
}

async function join(): Promise<void> {
  joinButton.disabled = true
  setStatus('接続中…')
  let session: MoqSession | undefined
  try {
    session = await MoqSession.connect(urlInput.value.trim(), connectOptions())
    const me = `${randomName()}-${crypto.randomUUID().slice(0, 4)}`
    const namespace = [...ROOM_NAMESPACE, me]
    board = new Board()
    requestRedraw()
    const announcement = await session.announce(namespace)
    const writer = await session.publishTrack(namespace, TRACK, { retentionMs: RETENTION_MS })
    const watch = await session.watchNamespaces(ROOM_NAMESPACE)
    const opened: Joined = {
      session,
      me,
      announcement,
      writer,
      watch,
      peers: new Map(),
      writing: Promise.resolve(),
      opsInGroup: 0
    }
    joined = opened
    // The track's first group, so a peer that joins before this page draws has a group to read.
    startGroup(opened)
    void followPeers(opened)
    void session.closed.then(({ reason }) => {
      if (joined === opened) {
        void leave(`切断されました: ${reason}`)
      }
    })
    setEnabled(true)
    renderParticipants()
    setStatus(`参加中: ${me}`)
  } catch (error) {
    session?.close()
    joinButton.disabled = false
    setStatus(`参加できませんでした: ${getErrorMessage(error)}`)
  }
}

async function leave(status: string): Promise<void> {
  const left = joined
  joined = undefined
  drawing = undefined
  setEnabled(false)
  setStatus(status)
  renderParticipants()
  if (!left) {
    return
  }
  await left.watch.close().catch(() => undefined)
  await Promise.allSettled([...left.peers.values()].map((subscription) => subscription.close()))
  await left.announcement.close().catch(() => undefined)
  left.session.close()
}

/// Reads every peer in the room, those already present included.
async function followPeers(target: Joined): Promise<void> {
  try {
    for await (const { kind, namespace } of target.watch) {
      const peer = namespace.at(-1)
      if (joined !== target || namespace.length !== ROOM_NAMESPACE.length + 1 || !peer || peer === target.me) {
        continue
      }
      await target.peers
        .get(peer)
        ?.close()
        .catch(() => undefined)
      target.peers.delete(peer)
      if (kind === 'announced') {
        void readPeer(target, peer, namespace)
      } else {
        // The peer's track is gone, so this page's next group must carry the peer's strokes for late joiners.
        startGroup(target)
      }
      renderParticipants()
    }
  } catch (error) {
    console.warn('[whiteboard] namespace watch ended', error)
  }
}

/// The peer's current group first, which starts with its snapshot, then its operations as they come.
async function readPeer(target: Joined, peer: string, namespace: string[]): Promise<void> {
  try {
    const subscription = await target.session.subscribeCurrentGroup(namespace, TRACK)
    if (joined !== target) {
      await subscription.close()
      return
    }
    target.peers.set(peer, subscription)
    renderParticipants()
    for await (const object of subscription) {
      const op = decodeOp(object.objectPayload)
      if (op && board.apply(op)) {
        requestRedraw()
      }
    }
  } catch (error) {
    console.warn(`[whiteboard] reading ${peer} ended`, error)
  }
}

function publish(target: Joined, op: BoardOp): void {
  if (board.apply(op)) {
    requestRedraw()
  }
  send(target, op)
}

function send(target: Joined, op: BoardOp): void {
  write(target, op, false)
  target.opsInGroup += 1
  if (target.opsInGroup >= OPS_PER_GROUP) {
    startGroup(target)
  }
}

function startGroup(target: Joined): void {
  write(target, board.snapshot(), true)
  target.opsInGroup = 0
}

function write(target: Joined, op: BoardOp, newGroup: boolean): void {
  const payload = encodeOp(op)
  target.writing = target.writing.then(() =>
    target.writer.write(payload, newGroup, undefined).catch((error: unknown) => {
      console.warn('[whiteboard] write failed', error)
    })
  )
}

function pointOf(event: PointerEvent): [number, number] {
  const rect = canvas.getBoundingClientRect()
  const clamp = (value: number) => Math.round(Math.min(1, Math.max(0, value)) * 10_000) / 10_000
  return [clamp((event.clientX - rect.left) / rect.width), clamp((event.clientY - rect.top) / rect.height)]
}

function onPointerDown(event: PointerEvent): void {
  if (!joined || event.button !== 0) {
    return
  }
  canvas.setPointerCapture(event.pointerId)
  if (tool === 'eraser') {
    eraseAt(joined, event)
    return
  }
  const id = strokeId(board.nextStamp(joined.me))
  const points = pointOf(event)
  publish(joined, { kind: 'stroke', stroke: { id, color: colorInput.value, width: Number(widthInput.value), points } })
  drawing = { id, points, sent: points.length }
}

function onPointerMove(event: PointerEvent): void {
  if (!joined || !canvas.hasPointerCapture(event.pointerId)) {
    return
  }
  if (tool === 'eraser') {
    eraseAt(joined, event)
    return
  }
  if (!drawing) {
    return
  }
  // Drawn at once; `flushPoints` sends the new points in batches.
  const from = drawing.points.length
  const points = pointOf(event)
  drawing.points.push(...points)
  if (board.apply({ kind: 'points', id: drawing.id, from, points })) {
    requestRedraw()
  }
}

function endStroke(): void {
  flushPoints()
  drawing = undefined
}

function flushPoints(): void {
  if (!joined || !drawing || drawing.points.length <= drawing.sent) {
    return
  }
  const from = drawing.sent
  drawing.sent = drawing.points.length
  // Sent without applying: `onPointerMove` put the points on this page's board.
  send(joined, { kind: 'points', id: drawing.id, from, points: drawing.points.slice(from) })
}

function eraseAt(target: Joined, event: PointerEvent): void {
  const [x, y] = pointOf(event)
  const radius = ERASER_RADIUS_PX / canvas.clientWidth
  const ids = board
    .visibleStrokes()
    .filter((stroke) => touches(stroke, x, y * ASPECT, radius))
    .map((stroke) => stroke.id)
  if (ids.length > 0) {
    publish(target, { kind: 'erase', ids })
  }
}

/// Whether a segment of `stroke` passes within `radius` of (x, y), in units of the board's width.
function touches(stroke: Stroke, x: number, y: number, radius: number): boolean {
  const reach = radius + stroke.width / 2 / canvas.clientWidth
  const at = (index: number): [number, number] => [stroke.points[index], stroke.points[index + 1] * ASPECT]
  for (let index = 0; index < stroke.points.length; index += 2) {
    const [ax, ay] = at(index)
    const [bx, by] = index + 2 < stroke.points.length ? at(index + 2) : [ax, ay]
    const dx = bx - ax
    const dy = by - ay
    const length = dx * dx + dy * dy
    const t = length === 0 ? 0 : Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / length))
    if (Math.hypot(x - (ax + t * dx), y - (ay + t * dy)) <= reach) {
      return true
    }
  }
  return false
}

function requestRedraw(): void {
  if (redrawPending) {
    return
  }
  redrawPending = true
  requestAnimationFrame(() => {
    redrawPending = false
    redraw()
  })
}

function redraw(): void {
  const ratio = window.devicePixelRatio || 1
  const width = canvas.clientWidth
  const height = canvas.clientHeight
  if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) {
    canvas.width = Math.round(width * ratio)
    canvas.height = Math.round(height * ratio)
  }
  const ctx = canvas.getContext('2d')!
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0)
  ctx.clearRect(0, 0, width, height)
  ctx.lineCap = 'round'
  ctx.lineJoin = 'round'
  const strokes = board.visibleStrokes()
  for (const stroke of strokes) {
    ctx.strokeStyle = stroke.color
    ctx.lineWidth = Math.min(64, Math.max(1, stroke.width))
    ctx.beginPath()
    ctx.moveTo(stroke.points[0] * width, stroke.points[1] * height)
    // A stroke of one point ends where it starts, which the round cap draws as a dot.
    const start = stroke.points.length > 2 ? 2 : 0
    for (let index = start; index < stroke.points.length; index += 2) {
      ctx.lineTo(stroke.points[index] * width, stroke.points[index + 1] * height)
    }
    ctx.stroke()
  }
  strokeCount.textContent = `${strokes.length} ストローク`
  strokeCount.dataset.count = strokes.length.toString()
}

function renderParticipants(): void {
  const names = joined ? [`${joined.me}（自分）`, ...[...joined.peers.keys()].sort()] : []
  participantList.replaceChildren(
    ...names.map((name) => {
      const item = document.createElement('li')
      item.textContent = name
      return item
    })
  )
  participantList.dataset.count = names.length.toString()
}
