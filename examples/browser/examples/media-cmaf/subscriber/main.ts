import { MoqSession, reportLog } from 'moq-wasm'
import { LivePlayer, formatLiveStats } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'
import { connectOptions } from '../../../utils/access'
import { element, parseTrackNamespace, setStatusText, initializeMediaExamplePage } from '../../../utils/page'
import { startPageTelemetry } from '../../../utils/pageTelemetry'

let session: MoqSession | undefined
const form = element<HTMLFormElement>('form')
const stage = element<HTMLDivElement>('stage')
const player = new LivePlayer({
  container: stage,
  callbacks: {
    onStateChange: renderPlayer,
    onLiveFrame: renderStats,
    onLog: (level, message) => {
      reportLog(level, 'media-cmaf-subscriber', message)
      console[level](`[CmafSubscriber] ${message}`)
    }
  }
})
startPageTelemetry('media-cmaf-subscriber', () => form.url.value)

function renderPlayer(): void {
  stage.toggleAttribute('data-started', player.state.started)
  const { started, catalogStatus, playbackStatus } = player.state
  setStatusText('catalog-track-status', catalogStatus.text)
  if (started) {
    setStatusText('subscriber-playback-status', playbackStatus.text)
  }
}

function renderStats(): void {
  const stats = player.stats()
  element('playout-stats').textContent =
    stats.bufferMs === undefined && stats.frameSize === undefined ? 'Waiting for media' : formatLiveStats(stats)
}

function initializeStatuses(): void {
  setStatusText('subscriber-connection-status', 'Not connected')
  setStatusText('catalog-track-status', 'Catalog not loaded yet')
  setStatusText('subscriber-playback-status', 'Playback idle')
}

element('sendSubscribeBtn').addEventListener('click', async () => {
  if (!session) {
    setStatusText('subscriber-playback-status', 'Connect first')
    return
  }
  await player.stop()
  await player.setPackaging('cmaf')
  // The audio has no CMAF track, so the player plays the video alone.
  await player.start(session, parseTrackNamespace(form['subscribe-track-namespace'].value))
})

element('closeBtn').addEventListener('click', async () => {
  await player.stop()
  session?.close()
  session = undefined
  initializeStatuses()
  renderStats()
  setStatusText('subscriber-connection-status', 'Disconnected')
})

element('connectBtn').addEventListener('click', async () => {
  const url = form.url.value
  session = await MoqSession.connect(url, connectOptions())
  setStatusText('subscriber-connection-status', `Connected: ${url}`)
})

initializeMediaExamplePage('subscribe-track-namespace')
initializeStatuses()
