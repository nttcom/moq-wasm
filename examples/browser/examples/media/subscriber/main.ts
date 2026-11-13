import { MoqSession, reportLog } from 'moq-wasm'
import { LivePlayer, formatLiveStats } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'
import { connectOptions } from '../../../utils/access'
import { element, parseTrackNamespace, setStatusText, initializeMediaExamplePage } from '../../../utils/page'
import { setUpCatalogPicker } from '../catalogPicker'
import { playerSamples, startPageTelemetry } from '../../../utils/pageTelemetry'

let session: MoqSession | undefined
const form = element<HTMLFormElement>('form')
const player = new LivePlayer({
  container: element<HTMLDivElement>('stage'),
  callbacks: {
    onStateChange: renderPlayer,
    onLiveFrame: renderStats,
    onLog: (level, message) => {
      reportLog(level, 'media-subscriber', message)
      console[level](`[MediaSubscriber] ${message}`)
    }
  }
})
startPageTelemetry(
  'media-subscriber',
  () => form.url.value,
  () => playerSamples(player)
)
const catalogPicker = setUpCatalogPicker(() => session, form)

function renderPlayer(): void {
  const { started, playbackStatus } = player.state
  if (started) {
    setStatusText('subscriber-playback-status', playbackStatus.text)
  }
}

function renderStats(): void {
  const stats = player.stats()
  setStatusText(
    'subscriber-receive-status',
    `Received video objects: ${stats.videoObjects}, audio objects: ${stats.audioObjects}`
  )
  element('playout-stats').textContent =
    stats.bufferMs === undefined && stats.frameSize === undefined ? 'Waiting for media' : formatLiveStats(stats)
}

function initializeStatuses(): void {
  setStatusText('subscriber-connection-status', 'Not connected')
  setStatusText('catalog-track-status', 'Catalog not loaded yet')
  setStatusText('subscriber-track-status', 'Subscription idle')
  setStatusText('subscriber-receive-status', 'Waiting for media objects')
  setStatusText('subscriber-playback-status', 'Playback idle')
}

element('sendSubscribeBtn').addEventListener('click', async () => {
  const { video, audio } = catalogPicker.pick
  if (!session) {
    setStatusText('subscriber-track-status', 'Connect first')
    return
  }
  if (!video.selected) {
    setStatusText('catalog-track-status', 'Select a video track from catalog first')
    return
  }
  const description = `video=${video.selected.name}, audio=${audio.selected?.name ?? 'none'}`
  await player.stop()
  // Without the picked tracks, the player subscribes the catalog itself and picks its own.
  await player.start(session, parseTrackNamespace(form['subscribe-track-namespace'].value), {
    video: [video.selected],
    audio: audio.selected ? [audio.selected] : []
  })
  setStatusText('subscriber-track-status', `Subscribed ${description}`)
})

element('closeBtn').addEventListener('click', async () => {
  await player.stop()
  session?.close()
  session = undefined
  catalogPicker.reset()
  initializeStatuses()
  renderStats()
  setStatusText('subscriber-connection-status', 'Disconnected')
  setStatusText('catalog-track-status', 'Disconnected')
})

element('connectBtn').addEventListener('click', async () => {
  const url = form.url.value
  session = await MoqSession.connect(url, connectOptions())
  setStatusText('subscriber-connection-status', `Connected: ${url}`)
})

initializeMediaExamplePage('subscribe-track-namespace')
initializeStatuses()
