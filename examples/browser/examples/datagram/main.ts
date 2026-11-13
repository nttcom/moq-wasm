import { type Announcement, MoqSession, type MoqSubscription, type TrackWriter } from 'moq-wasm'
import { connectOptions, withNamespaceRoot } from '../../utils/access'
import { setUpRelayUrlControls } from '../../utils/relayPicker'
import { element, getErrorMessage } from '../../utils/page'
import { startPageTelemetry } from '../../utils/pageTelemetry'
import {
  type CursorSample,
  type LatencyPoint,
  type TrackSummary,
  TrackStats,
  decodeCursorSample,
  encodeCursorSample
} from './cursorWire'

const LANES = ['stream', 'datagram'] as const
type Lane = (typeof LANES)[number]

/// A group per second at 60 Hz, so a lost packet holds back the rest of the stream's group.
const OBJECTS_PER_GROUP = 60
const SEND_INTERVAL_MS = 16
const WINDOW_MS = 10_000
const TRAIL_LENGTH = 24
const MIN_CHART_MS = 50

type Received = { stats: TrackStats; trail: CursorSample[] }

type Running = {
  session: MoqSession
  announcement: Announcement
  writers: Record<Lane, TrackWriter>
  subscriptions: MoqSubscription[]
  sendTimer: number
}

const urlInput = element<HTMLInputElement>('url')
const startButton = element<HTMLButtonElement>('startBtn')
const stopButton = element<HTMLButtonElement>('stopBtn')
const autoMove = element<HTMLInputElement>('autoMove')

startPageTelemetry('datagram', () => urlInput.value.trim())
setUpRelayUrlControls(urlInput, element('urlPresets'))

let running: Running | undefined
let received = newReceived()
/// In [0, 1] of the pad.
let pointer = { x: 0.5, y: 0.5 }
let seq = 0

for (const lane of LANES) {
  const pad = element<HTMLCanvasElement>(`pad-${lane}`)
  pad.addEventListener('pointermove', (event) => {
    const rect = pad.getBoundingClientRect()
    pointer = { x: clamp((event.clientX - rect.left) / rect.width), y: clamp((event.clientY - rect.top) / rect.height) }
    autoMove.checked = false
  })
}
startButton.addEventListener('click', () => void start())
stopButton.addEventListener('click', () => void stop('停止しました'))
requestAnimationFrame(draw)
setInterval(renderStats, 250)

function newReceived(): Record<Lane, Received> {
  return {
    stream: { stats: new TrackStats(WINDOW_MS), trail: [] },
    datagram: { stats: new TrackStats(WINDOW_MS), trail: [] }
  }
}

function setStatus(text: string): void {
  element('status').textContent = text
}

async function start(): Promise<void> {
  startButton.disabled = true
  setStatus('接続中…')
  let session: MoqSession | undefined
  try {
    session = await MoqSession.connect(urlInput.value.trim(), connectOptions())
    const namespace = withNamespaceRoot(['anon', 'datagram', crypto.randomUUID().slice(0, 8)])
    const announcement = await session.announce(namespace)
    const writers = {
      stream: await session.publishTrack(namespace, 'stream', { retentionMs: 0 }),
      datagram: await session.publishTrack(namespace, 'datagram', { retentionMs: 0, datagram: true })
    }
    received = newReceived()
    const subscriptions = await Promise.all(LANES.map((lane) => session!.subscribe(namespace, lane)))
    subscriptions.forEach((subscription, index) => void readLane(LANES[index], subscription))
    seq = 0
    const opened: Running = { session, announcement, writers, subscriptions, sendTimer: 0 }
    opened.sendTimer = window.setInterval(() => void send(opened), SEND_INTERVAL_MS)
    running = opened
    void session.closed.then(({ reason }) => {
      if (running === opened) {
        void stop(`切断されました: ${reason}`)
      }
    })
    stopButton.disabled = false
    setStatus(`送受信中: ${namespace.join('/')}`)
  } catch (error) {
    session?.close()
    startButton.disabled = false
    setStatus(`開始できませんでした: ${getErrorMessage(error)}`)
  }
}

async function stop(status: string): Promise<void> {
  const stopped = running
  running = undefined
  stopButton.disabled = true
  startButton.disabled = false
  setStatus(status)
  if (!stopped) {
    return
  }
  clearInterval(stopped.sendTimer)
  await Promise.allSettled(stopped.subscriptions.map((subscription) => subscription.close()))
  await stopped.announcement.close().catch(() => undefined)
  stopped.session.close()
}

async function send(target: Running): Promise<void> {
  if (autoMove.checked) {
    const t = performance.now() / 1000
    pointer = { x: 0.5 + 0.35 * Math.cos(t * 1.3), y: 0.5 + 0.35 * Math.sin(t * 2.1) }
  }
  const sample = { seq, ...pointer, sentAtMs: nowMs() }
  const newGroup = seq % OBJECTS_PER_GROUP === 0
  seq += 1
  const payload = encodeCursorSample(sample)
  await Promise.all(
    LANES.map((lane) =>
      target.writers[lane].write(payload, newGroup, undefined).catch((error: unknown) => {
        console.warn(`[datagram] ${lane} write failed`, error)
      })
    )
  )
}

async function readLane(lane: Lane, subscription: MoqSubscription): Promise<void> {
  try {
    for await (const object of subscription) {
      const sample = decodeCursorSample(object.objectPayload)
      if (!sample) {
        continue
      }
      const { stats, trail } = received[lane]
      stats.record(sample, nowMs())
      trail.push(sample)
      if (trail.length > TRAIL_LENGTH) {
        trail.shift()
      }
    }
  } catch (error) {
    console.warn(`[datagram] ${lane} subscription ended`, error)
  }
}

function nowMs(): number {
  return performance.timeOrigin + performance.now()
}

function clamp(value: number): number {
  return Math.min(1, Math.max(0, value))
}

function renderStats(): void {
  const now = nowMs()
  for (const lane of LANES) {
    renderSummary(element(`stats-${lane}`), received[lane].stats.summary(now))
  }
}

function renderSummary(list: HTMLElement, summary: TrackSummary): void {
  const ms = (value: number | undefined) => (value === undefined ? '–' : `${value.toFixed(1)} ms`)
  const rows: [string, string][] = [
    ['受信数', summary.received.toString()],
    ['欠落', summary.lost.toString()],
    ['順序入れ替わり', summary.reordered.toString()],
    ['遅延 (最新)', ms(summary.lastLatencyMs)],
    ['遅延 (中央値 / p95)', `${ms(summary.medianLatencyMs)} / ${ms(summary.p95LatencyMs)}`],
    ['最大受信間隔 (10 秒)', ms(summary.maxGapMs)]
  ]
  list.dataset.received = summary.received.toString()
  list.replaceChildren(
    ...rows.map(([term, value]) => {
      const row = document.createElement('div')
      const dt = document.createElement('dt')
      dt.textContent = term
      const dd = document.createElement('dd')
      dd.textContent = value
      row.append(dt, dd)
      return row
    })
  )
}

function draw(): void {
  const style = getComputedStyle(document.documentElement)
  const colors = {
    accent: style.getPropertyValue('--accent'),
    muted: style.getPropertyValue('--muted'),
    border: style.getPropertyValue('--border')
  }
  const now = nowMs()
  const latencies = LANES.map((lane) => received[lane].stats.latencies(now))
  const chartMaxMs = Math.max(MIN_CHART_MS, ...latencies.flat().map((point) => point.latencyMs)) * 1.1
  LANES.forEach((lane, index) => {
    drawPad(element<HTMLCanvasElement>(`pad-${lane}`), received[lane].trail, colors)
    drawChart(element<HTMLCanvasElement>(`chart-${lane}`), latencies[index], now, chartMaxMs, colors)
  })
  requestAnimationFrame(draw)
}

type Colors = { accent: string; muted: string; border: string }

/// Sizes the canvas to its CSS box and returns its 2D context in CSS pixels.
function context(canvas: HTMLCanvasElement): { ctx: CanvasRenderingContext2D; width: number; height: number } {
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
  return { ctx, width, height }
}

function drawPad(canvas: HTMLCanvasElement, trail: readonly CursorSample[], colors: Colors): void {
  const { ctx, width, height } = context(canvas)
  ctx.strokeStyle = colors.border
  ctx.lineWidth = 1
  for (let step = 1; step < 4; step += 1) {
    ctx.beginPath()
    ctx.moveTo((width * step) / 4, 0)
    ctx.lineTo((width * step) / 4, height)
    ctx.moveTo(0, (height * step) / 4)
    ctx.lineTo(width, (height * step) / 4)
    ctx.stroke()
  }
  trail.forEach((sample, index) => {
    const last = index === trail.length - 1
    ctx.globalAlpha = last ? 1 : ((index + 1) / trail.length) * 0.5
    ctx.fillStyle = colors.accent
    ctx.beginPath()
    ctx.arc(sample.x * width, sample.y * height, last ? 9 : 4, 0, Math.PI * 2)
    ctx.fill()
  })
  ctx.globalAlpha = 1
  ctx.strokeStyle = colors.muted
  ctx.lineWidth = 2
  ctx.beginPath()
  ctx.arc(pointer.x * width, pointer.y * height, 14, 0, Math.PI * 2)
  ctx.stroke()
}

function drawChart(
  canvas: HTMLCanvasElement,
  points: readonly LatencyPoint[],
  nowMs: number,
  maxMs: number,
  colors: Colors
): void {
  const { ctx, width, height } = context(canvas)
  ctx.strokeStyle = colors.border
  ctx.lineWidth = 1
  ctx.strokeRect(0.5, 0.5, width - 1, height - 1)
  ctx.fillStyle = colors.muted
  ctx.font = '11px sans-serif'
  ctx.fillText(`遅延 ${maxMs.toFixed(0)} ms`, 6, 14)
  ctx.fillText('過去 10 秒', width - 62, height - 6)
  ctx.strokeStyle = colors.accent
  ctx.lineWidth = 1.5
  ctx.beginPath()
  points.forEach((point, index) => {
    const x = width * (1 - (nowMs - point.receivedAtMs) / WINDOW_MS)
    const y = height * (1 - point.latencyMs / maxMs)
    if (index === 0) {
      ctx.moveTo(x, y)
    } else {
      ctx.lineTo(x, y)
    }
  })
  ctx.stroke()
}
