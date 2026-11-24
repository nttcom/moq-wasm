import { MoqSession } from 'moq-wasm'
import 'moq-wasm/element'
import { formatLiveStats } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'
import { connectOptions } from '../../../utils/access'
import { element, parseTrackNamespace, setStatusText, initializeMediaExamplePage } from '../../../utils/page'
import { startPageTelemetry } from '../../../utils/pageTelemetry'

let session: MoqSession | undefined
const form = element<HTMLFormElement>('form')
const stage = element<HTMLElementTagNameMap['moq-player']>('stage')
stage.addEventListener('statechange', renderPlayer)
stage.addEventListener('liveframe', renderStats)
startPageTelemetry('media-cmaf-subscriber', () => form.url.value)

function renderPlayer(): void {
  const state = stage.player?.state
  stage.toggleAttribute('data-started', state?.started ?? false)
  if (!state) {
    return
  }
  setStatusText('catalog-track-status', state.catalogStatus.text)
  if (state.started) {
    setStatusText('subscriber-playback-status', state.playbackStatus.text)
  }
}

function renderStats(): void {
  const stats = stage.player?.stats()
  element('playout-stats').textContent =
    !stats || (stats.bufferMs === undefined && stats.frameSize === undefined)
      ? 'Waiting for media'
      : formatLiveStats(stats)
}

function initializeStatuses(): void {
  setStatusText('subscriber-connection-status', 'Not connected')
  setStatusText('catalog-track-status', 'Catalog not loaded yet')
  setStatusText('subscriber-playback-status', 'Playback idle')
}

element('sendSubscribeBtn').addEventListener('click', () => {
  if (!session) {
    setStatusText('subscriber-playback-status', 'Connect first')
    return
  }
  // The audio has no CMAF track, so the player plays the video alone.
  stage.setAttribute('namespace', parseTrackNamespace(form['subscribe-track-namespace'].value).join('/'))
  stage.session = session
})

element('closeBtn').addEventListener('click', async () => {
  // Unsubscribes before the session closes; the element then disposes the stopped player.
  await stage.player?.stop()
  stage.session = undefined
  session?.close()
  session = undefined
  initializeStatuses()
  renderStats()
  stage.removeAttribute('data-started')
  setStatusText('subscriber-connection-status', 'Disconnected')
})

element('connectBtn').addEventListener('click', async () => {
  const url = form.url.value
  session = await MoqSession.connect(url, connectOptions())
  setStatusText('subscriber-connection-status', `Connected: ${url}`)
})

initializeMediaExamplePage('subscribe-track-namespace')
initializeStatuses()
