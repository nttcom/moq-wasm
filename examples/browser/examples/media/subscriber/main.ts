import { MoqtClientWrapper } from '@moqt/moqtClient'
import { LocLive, formatLiveStats } from '@player/locLive'
import { getResolvedMediaVideoCodec } from '../catalog'
import { element, parseTrackNamespace, setStatusText, initializeMediaExamplePage } from '../../../utils/page'
import { setUpCatalogPicker } from '../catalogPicker'
import type { MediaKind } from '@player/trackContext'

const moqtClient = new MoqtClientWrapper()
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
const catalogPicker = setUpCatalogPicker(moqtClient, form, (kind, track) =>
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
  setStatusText('subscriber-setup-status', 'Setup not sent')
  setStatusText('catalog-track-status', 'Catalog not loaded yet')
  setStatusText('subscriber-track-status', 'Subscription idle')
  setStatusText('subscriber-receive-status', 'Waiting for media objects')
  setStatusText('subscriber-playback-status', 'Playback idle')
}

function sendSetupButtonClickHandler(): void {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const versions = new BigUint64Array([0xff00000en])
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

    await moqtClient.sendClientSetup(versions, maxSubscribeId)
    setStatusText('subscriber-setup-status', 'Setup acknowledged')
  })
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
    const videoSubscribeId = BigInt(form['video-subscribe-id'].value)
    const audioSubscribeId = BigInt(form['audio-subscribe-id'].value)

    if (!selectedVideoTrack) {
      setStatusText('catalog-track-status', 'Select a video track from catalog first')
      return
    }

    const videoTrackAlias = (
      await moqtClient.subscribe(trackNamespace, selectedVideoTrack, 'secret', {
        requestId: videoSubscribeId,
        onPublishDone: () => setStatusText('subscriber-track-status', 'Publisher ended the video track')
      })
    ).subscribeOk.trackAlias
    form['video-track-alias'].value = videoTrackAlias.toString()
    setupClientObjectCallbacks('video', videoTrackAlias)

    if (selectedAudioTrack) {
      const audioTrackAlias = (
        await moqtClient.subscribe(trackNamespace, selectedAudioTrack, 'secret', {
          requestId: audioSubscribeId,
          onPublishDone: () => setStatusText('subscriber-track-status', 'Publisher ended the audio track')
        })
      ).subscribeOk.trackAlias
      form['audio-track-alias'].value = audioTrackAlias.toString()
      setupClientObjectCallbacks('audio', audioTrackAlias)
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

function setupClientObjectCallbacks(kind: MediaKind, trackAlias: bigint): void {
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (groupId, subgroupStreamObject) => {
    receivedObjectCounts[kind] += 1
    setStatusText(
      'subscriber-receive-status',
      `Received video objects: ${receivedObjectCounts.video}, audio objects: ${receivedObjectCounts.audio}`
    )
    if (kind === 'video') {
      setStatusText('subscriber-track-status', `Subscribed ${describeSelectedTracks()}`)
    }
    console.debug(`[MediaSubscriber] recv ${kind} object`, {
      trackAlias: trackAlias.toString(),
      groupId,
      objectId: subgroupStreamObject.objectId,
      payloadLength: subgroupStreamObject.objectPayloadLength,
      status: subgroupStreamObject.objectStatus,
      loc: subgroupStreamObject.locHeader
    })
    locLive.push(kind, groupId, subgroupStreamObject)
  })
}

moqtClient.setOnServerSetupHandler((serverSetup: any) => {
  console.log({ serverSetup })
  setStatusText('subscriber-setup-status', 'Setup acknowledged')
})

moqtClient.setOnSubscribeResponseHandler((subscribeResponse) => {
  console.log({ subscribeResponse })
})

function setupCloseButtonHandler(): void {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    await moqtClient.disconnect()
    moqtClient.clearSubgroupObjectHandlers()
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
  sendSetupButtonClickHandler()
  sendSubscribeButtonClickHandler()
  setupCloseButtonHandler()
}

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const url = form.url.value

  await moqtClient.connect(url, { sendSetup: false })
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
