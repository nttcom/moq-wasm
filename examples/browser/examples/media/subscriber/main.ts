import { MoqSession } from 'moq-wasm'
import 'moq-wasm/element'
import { formatLiveStats } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'
import { connectOptions } from '../../../utils/access'
import { element, parseTrackNamespace, setStatusText, initializeMediaExamplePage } from '../../../utils/page'
import { setUpCatalogPicker } from '../catalogPicker'
import { startPageTelemetry } from '../../../utils/pageTelemetry'

let session: MoqSession | undefined
const form = element<HTMLFormElement>('form')
const stage = element<HTMLElementTagNameMap['moq-player']>('stage')
stage.addEventListener('statechange', renderPlayer)
stage.addEventListener('liveframe', renderStats)
startPageTelemetry('media-subscriber', () => form.url.value)
const catalogPicker = setUpCatalogPicker(() => session, form)

function renderPlayer(): void {
  const state = stage.player?.state
  stage.toggleAttribute('data-started', state?.started ?? false)
  if (state?.started) {
    setStatusText('subscriber-playback-status', state.playbackStatus.text)
  }
}

function renderStats(): void {
  const stats = stage.player?.stats()
  setStatusText(
    'subscriber-receive-status',
    `Received video objects: ${stats?.videoObjects ?? 0}, audio objects: ${stats?.audioObjects ?? 0}`
  )
  element('playout-stats').textContent =
    !stats || (stats.bufferMs === undefined && stats.frameSize === undefined)
      ? 'Waiting for media'
      : formatLiveStats(stats)
}

function initializeStatuses(): void {
  setStatusText('subscriber-connection-status', 'Not connected')
  setStatusText('catalog-track-status', 'Catalog not loaded yet')
  setStatusText('subscriber-track-status', 'Subscription idle')
  setStatusText('subscriber-receive-status', 'Waiting for media objects')
  setStatusText('subscriber-playback-status', 'Playback idle')
}

element('sendSubscribeBtn').addEventListener('click', () => {
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
  // Without the picked tracks, the player subscribes the catalog itself and picks its own.
  stage.tracks = { video: [video.selected], audio: audio.selected ? [audio.selected] : [] }
  stage.setAttribute('namespace', parseTrackNamespace(form['subscribe-track-namespace'].value).join('/'))
  stage.session = session
  setStatusText('subscriber-track-status', `Subscribed ${description}`)
})

element('closeBtn').addEventListener('click', async () => {
  // Unsubscribes before the session closes; the element then disposes the stopped player.
  await stage.player?.stop()
  stage.session = undefined
  session?.close()
  session = undefined
  catalogPicker.reset()
  initializeStatuses()
  renderStats()
  stage.removeAttribute('data-started')
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
