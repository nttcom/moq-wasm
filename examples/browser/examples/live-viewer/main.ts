import { MoqtClientWrapper } from '@moqt/moqtClient'
import { CLOUD_RELAY_PRESETS, LOAD_BALANCED_RELAY_PRESET } from '../../utils/relayPresets'
import { MEDIA_CATALOG_TRACK_NAME } from '../media/catalog'
import { getErrorMessage, initializeMediaExamplePage, parseTrackNamespace, setStatus } from '../media/common'
import type { LivePictureKind } from '@player/livePictureSink'
import { PlayerControls } from '@player/ui/playerControls'
import { Mp4Publisher } from './mp4Publisher'
import { PlaybackCharts, SAMPLE_INTERVAL_MS } from './playbackCharts'
import { LivePlayer, type LivePlayerState, formatSyncOffset } from '@player/livePlayer'
import { PublishPreview } from './publishPreview'
import {
  DEFAULT_WINDOW_SECONDS,
  StreamMonitor,
  type StreamRecord,
  renderDeliveryGrid,
  renderIdleStreamMonitor,
  renderStreamMonitor,
  streamKbps,
  summarizeStreams
} from './streamMonitor'

const AUTH_INFO = 'secret'

const moqtClient = new MoqtClientWrapper()
const streamMonitor = new StreamMonitor()
const stage = element<HTMLDivElement>('stage')
let streamWindowSeconds = DEFAULT_WINDOW_SECONDS
const player = new LivePlayer({
  client: moqtClient,
  authInfo: AUTH_INFO,
  container: stage,
  callbacks: { onStateChange: renderPlayer, onLiveFrame: renderVideoStats, onLog: appendLog },
  deliveryObserver: streamMonitor,
  /// `?livePicture=canvas` forces the canvas sink, to see the Safari path in Chrome.
  livePicture: (new URLSearchParams(location.search).get('livePicture') as LivePictureKind | null) ?? undefined
})
const controls = new PlayerControls(stage, player, appendLog)
controls.seekbar.setAttribute('aria-describedby', 'seek-hint')
const playbackCharts = new PlaybackCharts(element<HTMLElement>('playback-charts'))
setInterval(() => {
  if (player.state.started) {
    const stats = player.stats()
    const buffering = stats.bufferMs !== undefined
    playbackCharts.push({
      delayMs: stats.viewerDelayMs,
      bufferMs: stats.bufferMs,
      targetMs: buffering ? stats.targetBufferMs : undefined,
      outputLatencyMs: buffering ? stats.outputLatencyMs : undefined,
      spreadMs: buffering ? stats.arrivalSpreadMs : undefined,
      kbps: stats.receivedKbps,
      syncMs: stats.syncOffsetMs
    })
  }
}, SAMPLE_INTERVAL_MS)
const mp4Publisher = new Mp4Publisher(
  { onStatus: (text, state) => setStatus('publish-status', text, state), onLog: appendLog },
  new PublishPreview(element<HTMLCanvasElement>('publish-preview'))
)

for (const preset of [LOAD_BALANCED_RELAY_PRESET, ...CLOUD_RELAY_PRESETS]) {
  const button = document.createElement('button')
  button.type = 'button'
  button.dataset.url = preset.value
  button.textContent = `Cloud ${preset.label}`
  button.title = preset.helper
  element('urlPresets').appendChild(button)
}
initializeMediaExamplePage('namespace')
element<HTMLButtonElement>('watchBtn').addEventListener('click', () => void watchStream())
element<HTMLButtonElement>('stopBtn').addEventListener('click', () => void stopStream())
element<HTMLButtonElement>('publishBtn').addEventListener('click', () => void publishMp4())
element<HTMLButtonElement>('stopPublishBtn').addEventListener('click', () => void mp4Publisher.stop())
element<HTMLButtonElement>('deliveryToggleBtn').addEventListener('click', () => {
  const overlay = element<HTMLDivElement>('delivery-overlay')
  overlay.hidden = !overlay.hidden
  element<HTMLButtonElement>('deliveryToggleBtn').setAttribute('aria-pressed', String(!overlay.hidden))
})
moqtClient.setOnSubgroupHeaderHandler((header) => streamMonitor.opened(header.trackAlias, header.groupId))
element<HTMLInputElement>('stream-gops').addEventListener('input', (event) => {
  const keptGroups = Number((event.target as HTMLInputElement).value) || 1
  streamMonitor.setKeptGroups(keptGroups)
  mp4Publisher.sentStreams.setKeptGroups(keptGroups)
})
element<HTMLInputElement>('stream-window').addEventListener('input', (event) => {
  streamWindowSeconds = Number((event.target as HTMLInputElement).value) || DEFAULT_WINDOW_SECONDS
})
requestAnimationFrame(function renderStreamsEachFrame() {
  renderStreams()
  requestAnimationFrame(renderStreamsEachFrame)
})

async function watchStream(): Promise<void> {
  try {
    await stopStream()
    const url = element<HTMLInputElement>('url').value.trim()
    await moqtClient.connect(url)
    setStatus('connection-status', `Connected: ${url}`, 'ok')
    appendLog('info', `connected to ${url}`)
    await player.start(trackNamespace())
  } catch (error) {
    setStatus('connection-status', `Failed: ${getErrorMessage(error)}`, 'error')
    appendLog('error', getErrorMessage(error))
  }
}

async function stopStream(): Promise<void> {
  playbackCharts.reset()
  streamMonitor.reset()
  await player.stop()
  if (moqtClient.getConnectionStatus()) {
    await moqtClient.disconnect()
  }
  setStatus('connection-status', 'Not connected', 'idle')
}

async function publishMp4(): Promise<void> {
  const file = element<HTMLInputElement>('mp4-file').files?.[0]
  if (!file) {
    setStatus('publish-status', 'Choose an MP4 file first', 'error')
    return
  }
  setStatus('publish-status', `Opening ${file.name}`, 'idle')
  try {
    await mp4Publisher.start({
      file,
      url: element<HTMLInputElement>('url').value.trim(),
      namespace: trackNamespace(),
      authInfo: AUTH_INFO,
      loop: element<HTMLInputElement>('mp4-loop').checked
    })
  } catch (error) {
    setStatus('publish-status', `Publish failed: ${getErrorMessage(error)}`, 'error')
    appendLog('error', `publish: ${getErrorMessage(error)}`)
  }
}

function renderPlayer(): void {
  const state = player.state
  setStatus('catalog-status', state.catalogStatus.text, state.catalogStatus.state)
  setStatus('playback-status', state.playbackStatus.text, state.playbackStatus.state)
  setStatus('rewind-status', state.rewindStatus.text, state.rewindStatus.state)
  controls.render()
}

function renderVideoStats(): void {
  const stats = player.stats()
  const delay = stats.viewerDelayMs === undefined ? '' : ` · delay ${Math.round(stats.viewerDelayMs)} ms`
  const buffer =
    stats.bufferMs === undefined
      ? ''
      : ` · buffer ${Math.round(stats.bufferMs)} ms (target ${Math.round(stats.targetBufferMs)})`
  controls.renderBuffer(stats)
  const size = stats.frameSize ? `${stats.frameSize.width}x${stats.frameSize.height}` : ''
  element<HTMLSpanElement>('video-stats').textContent =
    `${size}${delay}${buffer} · ${Math.round(stats.receivedKbps)} kbps · ${stats.videoObjects} objects · A/V ${formatSyncOffset(stats.syncOffsetMs)} · audio breaks ${stats.audioBreaks} · video ${stats.videoDrops} · shed ${Math.round(stats.shedMs)} ms`
}

function renderPublishStreams(state: LivePlayerState): void {
  element<HTMLElement>('publish-live').style.display = mp4Publisher.publishing ? '' : 'none'
  const viewerDelayMs = player.stats().viewerDelayMs
  element<HTMLSpanElement>('publish-latency').textContent =
    state.started && state.packaging === 'loc' && state.mode === 'live' && viewerDelayMs !== undefined
      ? `viewer delay ${Math.round(viewerDelayMs)} ms`
      : 'viewer delay -'
  if (!mp4Publisher.publishing) {
    return
  }
  const now = Date.now()
  const records = mp4Publisher.sentStreams.snapshot()
  renderStreamMonitor(
    element<SVGSVGElement>('publish-stream-monitor'),
    records,
    mp4Publisher.sentStreams.slotsPerTrack(),
    streamWindowSeconds,
    [],
    now
  )
  element<HTMLSpanElement>('publish-stream-stats').textContent = records.length
    ? `${summarizeStreams(records, [], now)} · ${Math.round(streamKbps(records, streamWindowSeconds, now))} kbps`
    : 'no subscriber yet'
}

function renderStreams(): void {
  const state = player.state
  renderPublishStreams(state)
  const reviewGrid = element<SVGSVGElement>('delivery-grid-review')
  const reviewTimeline = element<SVGSVGElement>('stream-monitor-review')
  if (!state.started) {
    renderIdleStreamMonitor(element<SVGSVGElement>('stream-monitor'))
    renderIdleStreamMonitor(element<SVGSVGElement>('delivery-grid'))
    reviewGrid.style.display = 'none'
    reviewTimeline.style.display = 'none'
    element<HTMLSpanElement>('stream-stats').textContent = '-'
    return
  }
  const reviewing = state.mode === 'review'
  const { liveVideoAlias, liveAudioAlias, mediaTimelineTrackName, reviewFetchWindows } = state.delivery
  const now = Date.now()
  const records = streamMonitor.snapshot()
  const liveRecords = records.filter(
    (record) =>
      record.kind === 'subscribe' &&
      (!reviewing || record.track === MEDIA_CATALOG_TRACK_NAME || record.track === mediaTimelineTrackName)
  )
  const fetchRecords = records.filter((record) => record.kind === 'fetch')
  const playheads = streamMonitor.currentPlayheads()
  const livePlayhead = playheads.find((playhead) => playhead.kind === 'subscribe')
  const reviewPlayhead = playheads.find((playhead) => playhead.kind === 'fetch')
  renderStreamMonitor(
    element<SVGSVGElement>('stream-monitor'),
    liveRecords,
    streamMonitor.slotsPerTrack(),
    streamWindowSeconds,
    livePlayhead ? [livePlayhead] : [],
    now
  )
  element<HTMLSpanElement>('stream-stats').textContent = summarizeStreams(records, playheads, now)
  reviewTimeline.style.display = reviewing && fetchRecords.length > 0 ? '' : 'none'
  if (reviewing && fetchRecords.length > 0) {
    renderStreamMonitor(
      reviewTimeline,
      fetchRecords,
      streamMonitor.slotsPerTrack(),
      streamWindowSeconds,
      reviewPlayhead ? [reviewPlayhead] : [],
      latestFetchActivity(fetchRecords, now)
    )
  }
  const liveGrid = element<SVGSVGElement>('delivery-grid')
  liveGrid.style.display = reviewing ? 'none' : ''
  if (!reviewing) {
    renderDeliveryGrid(
      liveGrid,
      records,
      [
        { label: 'audio', kind: 'subscribe', trackAlias: liveAudioAlias },
        { label: 'video', kind: 'subscribe', trackAlias: liveVideoAlias }
      ],
      streamMonitor.slotsPerTrack(),
      livePlayhead
    )
  }
  reviewGrid.style.display = reviewing ? '' : 'none'
  if (reviewing) {
    const fetchIds =
      reviewFetchWindows.find((window) => window.video === reviewPlayhead?.trackAlias) ?? reviewFetchWindows[0] ?? {}
    renderDeliveryGrid(
      reviewGrid,
      records,
      [
        { label: 'fetch audio', kind: 'fetch', trackAlias: fetchIds.audio, cadenceAlias: liveAudioAlias },
        { label: 'fetch video', kind: 'fetch', trackAlias: fetchIds.video, cadenceAlias: liveVideoAlias }
      ],
      streamMonitor.slotsPerTrack(),
      reviewPlayhead
    )
  }
}

/// A FETCH arrives in a burst well before it is played, so the review
/// timeline ends at the newest FETCH activity rather than at the wall clock.
function latestFetchActivity(records: StreamRecord[], now: number): number {
  return Math.max(...records.map((record) => record.finishedAt ?? now))
}

function trackNamespace(): string[] {
  return parseTrackNamespace(element<HTMLInputElement>('namespace').value)
}

function appendLog(level: 'info' | 'warn' | 'error', message: string): void {
  const panel = document.getElementById('logPanel')
  if (!panel) {
    return
  }

  const entry = document.createElement('div')
  entry.className = `log-entry log-${level}`
  entry.textContent = `${new Date().toLocaleTimeString()} ${message}`
  panel.prepend(entry)
}

function element<T extends Element>(id: string): T {
  const found = document.getElementById(id)
  if (!found) {
    throw new Error(`missing element: ${id}`)
  }
  return found as Element as T
}
