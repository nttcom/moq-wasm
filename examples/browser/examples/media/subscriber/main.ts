import { MoqSession, type MoqSubscription, type ReceivedObject } from 'moq-wasm'
import { LocLive, formatLiveStats } from 'moq-wasm/media/player/locLive'
import { getResolvedMediaVideoCodec } from '../catalog'
import { element, parseTrackNamespace, setStatusText, initializeMediaExamplePage } from '../../../utils/page'
import { setUpCatalogPicker } from '../catalogPicker'
import { readObjects } from '../readObjects'
import type { MediaKind } from 'moq-wasm/media/player/trackContext'

let session: MoqSession | undefined
const form = element<HTMLFormElement>('form')

const PLAYOUT_STATS_INTERVAL_MS = 1000

const videoElement = document.getElementById('video') as HTMLVideoElement
const canvasElement = document.getElementById('canvas') as HTMLCanvasElement
const locLive = new LocLive(videoElement, canvasElement, undefined, {
  onPresented: () => {},
  onFrameShown: (ids) => {
    if (ids) {
      setPlaybackObjectPosition('video', ids.groupId, ids.objectId)
    }
  }
})
videoElement.hidden = locLive.picture.element !== videoElement
canvasElement.hidden = locLive.picture.element !== canvasElement

const receivedObjectCounts: Record<MediaKind, number> = { video: 0, audio: 0 }
const catalogPicker = setUpCatalogPicker(
  () => session,
  form,
  (kind, track) =>
    locLive.configureTrack(
      kind,
      kind === 'video' ? { ...track, codec: track.codec ?? getResolvedMediaVideoCodec() } : track
    )
)

function setPlaybackObjectStatus(kind: 'video', text: string): void {
  const element = document.getElementById(`${kind}-playback-object`)
  if (!element) {
    return
  }
  element.textContent = text
}

function setPlaybackObjectPosition(kind: 'video', groupId: bigint, objectId: bigint): void {
  setPlaybackObjectStatus(kind, `groupId=${groupId.toString()} objectId=${objectId.toString()}`)
}

function renderPlayoutStats(): void {
  const element = document.getElementById('playout-stats')
  if (!element) {
    return
  }
  const stats = locLive.stats()
  element.textContent =
    stats.bufferMs === undefined && stats.frameSize === undefined ? 'Waiting for media' : formatLiveStats(stats)
}

function initializeStatuses(): void {
  setStatusText('subscriber-connection-status', 'Not connected')
  setStatusText('catalog-track-status', 'Catalog not loaded yet')
  setStatusText('subscriber-track-status', 'Subscription idle')
  setStatusText('subscriber-receive-status', 'Waiting for media objects')
  setStatusText('subscriber-playback-status', 'Playback idle')
}

function describeSelectedTracks(): string {
  const { video, audio } = catalogPicker.pick
  return `video=${video.selected?.name}, audio=${audio.selected?.name ?? 'none'}`
}

function sendSubscribeButtonClickHandler(): void {
  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement
  sendSubscribeBtn.addEventListener('click', async () => {
    const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
    const selectedVideoTrack = catalogPicker.pick.video.selected?.name
    const selectedAudioTrack = catalogPicker.pick.audio.selected?.name

    if (!session) {
      setStatusText('subscriber-track-status', 'Connect first')
      return
    }
    if (!selectedVideoTrack) {
      setStatusText('catalog-track-status', 'Select a video track from catalog first')
      return
    }

    receiveObjects('video', await session.subscribe(trackNamespace, selectedVideoTrack))
    if (selectedAudioTrack) {
      receiveObjects('audio', await session.subscribe(trackNamespace, selectedAudioTrack))
    }
    setStatusText('subscriber-track-status', `Track subscribe requested: ${describeSelectedTracks()}`)
  })
}

function setupVideoPlaybackStatus(): void {
  const videoElement = document.getElementById('video') as HTMLVideoElement
  const updatePlaybackStatus = (label: string) => {
    setStatusText(
      'subscriber-playback-status',
      `${label}: readyState=${videoElement.readyState}, currentTime=${videoElement.currentTime.toFixed(2)}`
    )
  }

  videoElement.addEventListener('loadeddata', () => {
    updatePlaybackStatus('Loaded data')
  })
  videoElement.addEventListener('playing', () => {
    updatePlaybackStatus('Playing')
  })
  videoElement.addEventListener('timeupdate', () => {
    if (videoElement.currentTime > 0) {
      updatePlaybackStatus('Playing')
    }
  })
  videoElement.addEventListener('pause', () => {
    if (videoElement.currentTime === 0) {
      setStatusText('subscriber-playback-status', 'Playback paused')
    }
  })
}

function receiveObjects(kind: MediaKind, subscription: MoqSubscription): void {
  void subscription.done.then((done) => {
    if (done) {
      setStatusText('subscriber-track-status', `Publisher ended the ${kind} track`)
    }
  })
  readObjects(subscription, (object) => receiveObject(kind, object), `MediaSubscriber ${kind}`)
}

function receiveObject(kind: MediaKind, object: ReceivedObject): void {
  receivedObjectCounts[kind] += 1
  setStatusText(
    'subscriber-receive-status',
    `Received video objects: ${receivedObjectCounts.video}, audio objects: ${receivedObjectCounts.audio}`
  )
  if (kind === 'video') {
    setStatusText('subscriber-track-status', `Subscribed ${describeSelectedTracks()}`)
  }
  console.debug(`[MediaSubscriber] recv ${kind} object`, {
    groupId: object.groupId,
    objectId: object.objectId,
    payloadLength: object.objectPayloadLength,
    status: object.objectStatus,
    loc: object.locHeader
  })
  locLive.push(kind, object.groupId, object)
}

function setupCloseButtonHandler(): void {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    session?.close()
    session = undefined
    locLive.reset()
    setPlaybackObjectStatus('video', 'Waiting for objects')
    receivedObjectCounts.video = 0
    receivedObjectCounts.audio = 0
    catalogPicker.reset()
    initializeStatuses()
    setStatusText('subscriber-connection-status', 'Disconnected')
    setStatusText('catalog-track-status', 'Disconnected')
  })
}

function setupButtonHandlers(): void {
  sendSubscribeButtonClickHandler()
  setupCloseButtonHandler()
}

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const url = form.url.value

  session = await MoqSession.connect(url)
  receivedObjectCounts.video = 0
  receivedObjectCounts.audio = 0
  setStatusText('subscriber-connection-status', `Connected: ${url}`)
  setStatusText('subscriber-receive-status', 'Waiting for media objects')
})

initializeMediaExamplePage('subscribe-track-namespace')
initializeStatuses()
setupButtonHandlers()
setupVideoPlaybackStatus()
setPlaybackObjectStatus('video', 'Waiting for objects')
renderPlayoutStats()
window.setInterval(renderPlayoutStats, PLAYOUT_STATS_INTERVAL_MS)
