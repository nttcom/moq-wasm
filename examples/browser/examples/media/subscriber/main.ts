import { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../../pkg/moqt'
import { LocLive, formatLiveStats } from '@player/locLive'
import {
  AUDIO_LEVEL_ID,
  CAPTURE_TIMESTAMP_ID,
  VIDEO_CONFIG_ID,
  VIDEO_FRAME_MARKING_ID,
  type LocHeader
} from '../../../utils/media/loc'
import { getResolvedMediaVideoCodec } from '../catalog'
import { element, parseTrackNamespace, setStatusText, initializeMediaExamplePage } from '../../../utils/page'
import { extractCatalogAudioTracks, extractCatalogVideoTracks, type MediaCatalogTrack } from '../../../lib/msf/catalog'

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

let catalogVideoTracks: MediaCatalogTrack[] = []
let catalogAudioTracks: MediaCatalogTrack[] = []
let selectedVideoTrackName: string | null = null
let selectedAudioTrackName: string | null = null
let receivedVideoObjectCount = 0
let receivedAudioObjectCount = 0

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

function getCatalogTrackSelect(kind: 'video' | 'audio'): HTMLSelectElement | null {
  const id = kind === 'video' ? 'selected-video-track' : 'selected-audio-track'
  return document.getElementById(id) as HTMLSelectElement | null
}

function getCatalogTracks(kind: 'video' | 'audio'): MediaCatalogTrack[] {
  return kind === 'video' ? catalogVideoTracks : catalogAudioTracks
}

function getSelectedCatalogTrackName(kind: 'video' | 'audio'): string | null {
  return kind === 'video' ? selectedVideoTrackName : selectedAudioTrackName
}

function setSelectedCatalogTrackName(kind: 'video' | 'audio', trackName: string | null): void {
  if (kind === 'video') {
    selectedVideoTrackName = trackName
  } else {
    selectedAudioTrackName = trackName
  }
}

function setSelectedCatalogTrack(kind: 'video' | 'audio', trackName: string | null): void {
  setSelectedCatalogTrackName(kind, trackName)
  const select = getCatalogTrackSelect(kind)
  if (select && trackName) {
    select.value = trackName
  }
  if (!trackName) {
    return
  }
  if (kind === 'video') {
    const track = catalogVideoTracks.find((entry) => entry.name === trackName)
    locLive.configureTrack('video', {
      name: trackName,
      label: track?.label ?? trackName,
      codec: track?.codec ?? getResolvedMediaVideoCodec(),
      initData: track?.initData
    })
    return
  }
  const track = catalogAudioTracks.find((entry) => entry.name === trackName)
  if (track) {
    locLive.configureTrack('audio', track)
  }
}

function formatCatalogTrackLabel(track: MediaCatalogTrack, kind: 'video' | 'audio'): string {
  if (kind === 'video') {
    const resolution =
      typeof track.width === 'number' && typeof track.height === 'number' ? ` (${track.width}x${track.height})` : ''
    return `${track.label}${resolution}`
  }
  const details: string[] = []
  if (track.codec) {
    details.push(track.codec)
  }
  if (typeof track.samplerate === 'number') {
    details.push(`${track.samplerate}Hz`)
  }
  if (track.channelConfig) {
    details.push(track.channelConfig)
  }
  if (typeof track.bitrate === 'number') {
    details.push(`${Math.round(track.bitrate / 1000)}kbps`)
  }
  const metadata = details.length > 0 ? ` (${details.join(', ')})` : ''
  return `${track.label}${metadata}`
}

function renderCatalogTrackSelect(kind: 'video' | 'audio'): boolean {
  const select = getCatalogTrackSelect(kind)
  if (!select) {
    return false
  }
  select.innerHTML = ''
  const tracks = getCatalogTracks(kind)
  const isVideo = kind === 'video'
  const emptyMessage = isVideo ? 'Catalog video tracks are not loaded yet' : 'Catalog audio tracks are not loaded yet'

  if (!tracks.length) {
    const option = document.createElement('option')
    option.value = ''
    option.textContent = emptyMessage
    option.disabled = true
    option.selected = true
    select.appendChild(option)
    setSelectedCatalogTrack(kind, null)
    return false
  }

  let selectedTrackName = getSelectedCatalogTrackName(kind)
  if (!selectedTrackName || !tracks.some((track) => track.name === selectedTrackName)) {
    selectedTrackName = tracks[0].name
    setSelectedCatalogTrackName(kind, selectedTrackName)
  }

  for (const track of tracks) {
    const option = document.createElement('option')
    option.value = track.name
    option.textContent = formatCatalogTrackLabel(track, kind)
    option.selected = track.name === selectedTrackName
    select.appendChild(option)
  }
  setSelectedCatalogTrack(kind, selectedTrackName)
  return true
}

function renderCatalogTracks(): void {
  const hasVideoTracks = renderCatalogTrackSelect('video')
  const hasAudioTracks = renderCatalogTrackSelect('audio')

  if (!hasVideoTracks && !hasAudioTracks) {
    setStatusText('catalog-track-status', 'Catalog not loaded yet')
    return
  }
  setStatusText(
    'catalog-track-status',
    `Catalog loaded: video=${catalogVideoTracks.length}, audio=${catalogAudioTracks.length}`
  )
}

function setupCatalogSelectionHandler(): void {
  const videoSelect = getCatalogTrackSelect('video')
  if (videoSelect) {
    videoSelect.addEventListener('change', () => {
      const value = videoSelect.value.trim()
      setSelectedCatalogTrack('video', value.length > 0 ? value : null)
    })
  }
  const audioSelect = getCatalogTrackSelect('audio')
  if (audioSelect) {
    audioSelect.addEventListener('change', () => {
      const value = audioSelect.value.trim()
      setSelectedCatalogTrack('audio', value.length > 0 ? value : null)
    })
  }
}

function setupCatalogCallbacks(trackAlias: bigint): void {
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (groupId, subgroupStreamObject) => {
    const payloadBytes = subgroupStreamObject.objectPayload
    console.info('[MediaSubscriber] received catalog object', {
      trackAlias: trackAlias.toString(),
      groupId: groupId.toString(),
      subgroupId: subgroupStreamObject.subgroupId?.toString() ?? '0',
      objectIdDelta: subgroupStreamObject.objectIdDelta.toString(),
      payloadLength: subgroupStreamObject.objectPayloadLength
    })
    const payload = new TextDecoder().decode(payloadBytes)
    try {
      const parsed = parse_msf_catalog_json(payload)
      catalogVideoTracks = extractCatalogVideoTracks(parsed)
      catalogAudioTracks = extractCatalogAudioTracks(parsed)
      console.info('[MediaSubscriber] parsed catalog', {
        trackAlias: trackAlias.toString(),
        videoTracks: catalogVideoTracks.length,
        audioTracks: catalogAudioTracks.length
      })
      renderCatalogTracks()
    } catch (error) {
      console.error('[MediaSubscriber] failed to parse catalog', error)
      setStatusText('catalog-track-status', 'Catalog parse failed')
    }
  })
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

function sendCatalogSubscribeButtonClickHandler(): void {
  const sendCatalogSubscribeBtn = document.getElementById('sendCatalogSubscribeBtn') as HTMLButtonElement
  sendCatalogSubscribeBtn.addEventListener('click', async () => {
    const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
    const catalogTrackName = form['catalog-track-name'].value.trim()
    const catalogSubscribeId = BigInt(form['catalog-subscribe-id'].value)
    if (!catalogTrackName) {
      setStatusText('catalog-track-status', 'Catalog track is required')
      return
    }
    const catalogTrackAlias = (
      await moqtClient.subscribe(trackNamespace, catalogTrackName, 'secret', { requestId: catalogSubscribeId })
    ).subscribeOk.trackAlias
    form['catalog-track-alias'].value = catalogTrackAlias.toString()
    setStatusText('catalog-track-status', `Catalog subscribe requested: ${catalogTrackName}`)
    setupCatalogCallbacks(catalogTrackAlias)
  })
}

function describeSelectedTracks(): string {
  return `video=${selectedVideoTrackName}, audio=${selectedAudioTrackName ?? 'none'}`
}

function sendSubscribeButtonClickHandler(): void {
  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement
  sendSubscribeBtn.addEventListener('click', async () => {
    const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
    const selectedVideoTrack = selectedVideoTrackName
    const selectedAudioTrack = selectedAudioTrackName
    const videoSubscribeId = BigInt(form['video-subscribe-id'].value)
    const audioSubscribeId = BigInt(form['audio-subscribe-id'].value)

    if (!selectedVideoTrack) {
      setStatusText('catalog-track-status', 'Select a video track from catalog first')
      return
    }

    const videoTrackAlias = (
      await moqtClient.subscribe(trackNamespace, selectedVideoTrack, 'secret', { requestId: videoSubscribeId })
    ).subscribeOk.trackAlias
    form['video-track-alias'].value = videoTrackAlias.toString()
    setupClientObjectCallbacks('video', videoTrackAlias)

    if (selectedAudioTrack) {
      const audioTrackAlias = (
        await moqtClient.subscribe(trackNamespace, selectedAudioTrack, 'secret', { requestId: audioSubscribeId })
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

function setupClientObjectCallbacks(type: 'video' | 'audio', trackAlias: bigint): void {
  const alias = trackAlias

  if (type === 'audio') {
    moqtClient.setOnSubgroupObjectHandler(alias, (groupId, subgroupStreamObject) => {
      receivedAudioObjectCount += 1
      setStatusText(
        'subscriber-receive-status',
        `Received video objects: ${receivedVideoObjectCount}, audio objects: ${receivedAudioObjectCount}`
      )
      const locSummary = summarizeLocHeader(subgroupStreamObject.locHeader)
      if (locSummary.present && locSummary.extensionCount > 0) {
        console.debug('[MediaSubscriber] LoC object (audio)', {
          trackAlias: alias.toString(),
          groupId,
          objectId: subgroupStreamObject.objectId,
          loc: locSummary
        })
      }
      console.debug('[MediaSubscriber] recv audio object', {
        groupId,
        objectId: subgroupStreamObject.objectId,
        payloadLength: subgroupStreamObject.objectPayloadLength,
        status: subgroupStreamObject.objectStatus,
        loc: locSummary
      })
      locLive.push('audio', groupId, subgroupStreamObject)
    })
    return
  }

  moqtClient.setOnSubgroupObjectHandler(alias, (groupId, subgroupStreamObject) => {
    receivedVideoObjectCount += 1
    setStatusText(
      'subscriber-receive-status',
      `Received video objects: ${receivedVideoObjectCount}, audio objects: ${receivedAudioObjectCount}`
    )
    setStatusText('subscriber-track-status', `Subscribed ${describeSelectedTracks()}`)
    const locSummary = summarizeLocHeader(subgroupStreamObject.locHeader)
    if (locSummary.present && locSummary.extensionCount > 0) {
      console.debug('[MediaSubscriber] LoC object (video)', {
        trackAlias: alias.toString(),
        groupId,
        objectId: subgroupStreamObject.objectId,
        loc: locSummary
      })
    }
    console.debug('[MediaSubscriber] recv video object', {
      groupId,
      objectId: subgroupStreamObject.objectId,
      payloadLength: subgroupStreamObject.objectPayloadLength,
      status: subgroupStreamObject.objectStatus,
      loc: locSummary
    })

    locLive.push('video', groupId, subgroupStreamObject)
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
    catalogVideoTracks = []
    catalogAudioTracks = []
    selectedVideoTrackName = null
    selectedAudioTrackName = null
    setPlaybackObjectStatus('video', 'Waiting for objects')
    receivedVideoObjectCount = 0
    receivedAudioObjectCount = 0
    renderCatalogTracks()
    initializeStatuses()
    setStatusText('subscriber-connection-status', 'Disconnected')
    setStatusText('catalog-track-status', 'Disconnected')
  })
}

function setupButtonHandlers(): void {
  sendSetupButtonClickHandler()
  sendCatalogSubscribeButtonClickHandler()
  sendSubscribeButtonClickHandler()
  setupCatalogSelectionHandler()
  setupCloseButtonHandler()
}

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const url = form.url.value

  await moqtClient.connect(url, { sendSetup: false })
  receivedVideoObjectCount = 0
  receivedAudioObjectCount = 0
  setStatusText('subscriber-connection-status', `Connected: ${url}`)
  setStatusText('subscriber-receive-status', 'Waiting for media objects')
})

initializeMediaExamplePage('subscribe-track-namespace')
initializeStatuses()
setupButtonHandlers()
setupVideoPlaybackStatus()
renderCatalogTracks()
setPlaybackObjectStatus('video', 'Waiting for objects')
renderPlayoutStats()
window.setInterval(renderPlayoutStats, PLAYOUT_STATS_INTERVAL_MS)

function summarizeLocHeader(locHeader: unknown) {
  const extensions: LocHeader = Array.isArray(locHeader) ? locHeader : []
  const has = (id: number) => extensions.some((ext) => ext.id === id)
  return {
    present: Boolean(locHeader),
    extensionCount: extensions.length,
    hasCaptureTimestamp: has(CAPTURE_TIMESTAMP_ID),
    hasVideoConfig: has(VIDEO_CONFIG_ID),
    hasVideoFrameMarking: has(VIDEO_FRAME_MARKING_ID),
    hasAudioLevel: has(AUDIO_LEVEL_ID)
  }
}
