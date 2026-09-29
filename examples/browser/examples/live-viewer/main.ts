import { MoqtClientWrapper } from '@moqt/moqtClient'
import { CLOUD_RELAY_PRESETS, LOAD_BALANCED_RELAY_PRESET } from '../../utils/relayPresets'
import { parse_msf_catalog_json } from '../../pkg/moqt_client_wasm'
import {
  MEDIA_CATALOG_TRACK_NAME,
  extractCatalogAudioTracks,
  extractCatalogCmafTracks,
  extractCatalogMediaTimelineTracks,
  extractCatalogVideoTracks,
  type MediaCatalogTrack
} from '../media/catalog'
import { base64ToUint8Array } from '../../utils/media/base64'
import { monotonicUnixMicros } from '../../utils/media/clock'
import { postSubgroupObjectToWorker } from '../../utils/media/decoderWorker'
import { readLocHeader } from '../../utils/media/loc'
import {
  parseAudioChannelCount,
  postAudioCatalogToWorker,
  postVideoCatalogToWorker
} from '../../utils/media/decoderCatalog'
import { MseSink, type MseTrackSource } from '../../utils/media/mseSink'
import {
  getErrorMessage,
  initializeMediaExamplePage,
  parseTrackNamespace,
  setStatus,
  setStatusText
} from '../media/common'
import { BufferingSpinner } from './bufferingSpinner'
import { DEFAULT_PLAYOUT_DELAY_MS, LivePlayout } from './livePlayout'
import { type LivePictureKind, createLivePictureSink } from './livePictureSink'
import { MediaTimeline, formatElapsed } from './mediaTimeline'
import { Mp4Publisher } from './mp4Publisher'
import { PublishPreview } from './publishPreview'
import { ReviewPlayout } from './reviewPlayout'
import { GroupTimeline, type ReviewFrame, sortReviewFrames, toReviewFrame } from './rewind'
import {
  DEFAULT_WINDOW_SECONDS,
  type Playhead,
  StreamMonitor,
  type StreamRecord,
  renderDeliveryGrid,
  renderIdleStreamMonitor,
  renderStreamMonitor,
  streamKbps,
  summarizeStreams
} from './streamMonitor'

const AUTH_INFO = 'secret'
const ANNEX_B_FORMAT = 'annexb'
const TIMELINE_CAPACITY = 64
const REWIND_GROUP_COUNT = 4n
const FETCH_DEADLINE_MS = 8_000
const CLOSED_GROUP_POLL_MS = 200
const TRACK_STATUS_POLL_MS = 500
const AUDIO_GROUP_CLOSE_WAIT_MS = 2_000
const REVIEW_PLAYHEAD_STEP_US = 1_000_000
const CMAF_TRACK_SUFFIX = '_cmaf'
const REVIEW_BUFFER_AHEAD_SECONDS = 8
const REVIEW_DRAINED_SECONDS = 0.5
const REVIEW_VIDEO_AHEAD_FRAMES = 30
const REVIEW_HANDOVER_MICROS = 500_000
const MICROS_PER_SECOND = 1_000_000
const SKIP_SECONDS_BY_KEY: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowDown: -5, ArrowUp: 5 }
const MSE_ELEMENT_IDS = ['mse-a', 'mse-b', 'mse-c']
const POINTER_IDLE_MS = 2_500
const PRESENTATION_MARGIN_MS = 200

type Packaging = 'loc' | 'cmaf'

type MediaKind = 'video' | 'audio'

type SubgroupObject = Parameters<Parameters<MoqtClientWrapper['setOnSubgroupObjectHandler']>[1]>[1]

type SubscribeOk = Awaited<ReturnType<MoqtClientWrapper['subscribe']>>['subscribeOk']

type TrackSubscription = {
  requestId: bigint
  trackAlias: bigint
  name: string
  track: MediaCatalogTrack
}

type ReviewWindow = {
  start: bigint
  nextGroup: bigint
  frames: ReviewFrame[]
  audio: ReviewFrame[]
}

const moqtClient = new MoqtClientWrapper()
const videoDecoderWorker = new Worker(new URL('../../utils/media/decoders/videoDecoder.ts', import.meta.url), {
  type: 'module'
})
const audioDecoderWorker = new Worker(new URL('../../utils/media/decoders/audioDecoder.ts', import.meta.url), {
  type: 'module'
})
/// `?livePicture=canvas` forces the canvas sink, to see the Safari path in Chrome.
const livePictureSink = createLivePictureSink(
  element<HTMLVideoElement>('video'),
  element<HTMLCanvasElement>('live-canvas'),
  (picture) => notePresentedFrame(picture),
  (new URLSearchParams(location.search).get('livePicture') as LivePictureKind | null) ?? undefined
)
const livePlayout = new LivePlayout(
  showLiveFrame,
  () => appendLog('warn', 'playout clock re-anchored: scheduled video and audio were dropped'),
  (origin) =>
    videoDecoderWorker.postMessage({
      type: 'timeline',
      captureMicros: origin?.captureMicros,
      dueAtUnixMs: origin && performance.timeOrigin + origin.atMs
    })
)
const reviewPlayout = new ReviewPlayout(showReviewFrame, (message) => appendLog('error', message))
const mp4Publisher = new Mp4Publisher(
  { onStatus: (text, state) => setStatus('publish-status', text, state), onLog: appendLog },
  new PublishPreview(element<HTMLCanvasElement>('publish-preview'))
)
/// Live LOC frames carry their capture timestamp, so the moment one is shown
/// says how far the viewer runs behind the publisher on the same wall clock.
let viewerDelayMs: number | undefined
const bufferingSpinner = new BufferingSpinner(element('buffering'))

let videoTracks: MediaCatalogTrack[] = []
let audioTracks: MediaCatalogTrack[] = []
let cmafTracks: MediaCatalogTrack[] = []
let packaging: Packaging = 'loc'
let mse: MseSink | undefined
let reviewMse: MseSink | undefined
let reviewMseOpened = false
let visiblePicture: HTMLElement = element('video')
showPicture(livePictureSink.element)
/// MSE decodes from the first random access point, so after a MediaSource is
/// (re)opened live fragments are dropped until one starts a group.
let cmafAwaitingKeyframe = true
const unstampedGroups = new Set<bigint>()
const subscriptions = new Map<MediaKind, TrackSubscription>()
let catalogGroupId: bigint | undefined
let videoObjectCount = 0
let receivedKbps = 0
const timeline = new GroupTimeline(TIMELINE_CAPACITY)
const streamMonitor = new StreamMonitor()
let streamWindowSeconds = DEFAULT_WINDOW_SECONDS
let watching = false
const decodedFrameIds = new Map<number, Omit<Playhead, 'kind' | 'trackAlias' | 'captureMicros'>>()
const reviewFrameIds = new Map<number, Omit<Playhead, 'captureMicros'>>()
let reviewFetchIds: { video?: bigint; audio?: bigint } = {}
let reviewBehindSeconds = 0
let newestAudioGroupId: bigint | undefined
const mediaTimeline = new MediaTimeline()
let mediaTimelineTrackName: string | undefined
let mediaTimelineDepends: string[] = []
let reviewing = false
let liveForwardPaused = false
let liveForwardUpdate = Promise.resolve()
let reviewGeneration = 0
let reviewAnchorMicros: number | undefined
let reviewOriginMicros: number | undefined
let reviewPlayheadMicros: number | undefined
let seeking = false
let paused = false
let volume = 1
let pointerIdleTimer: ReturnType<typeof setTimeout> | undefined
const seekbar = element<HTMLInputElement>('seekbar')
const stage = element<HTMLDivElement>('stage')

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
element<HTMLSelectElement>('video-track').addEventListener('change', () => void resubscribe('video').then(openLiveMse))
element<HTMLSelectElement>('audio-track').addEventListener('change', () => void resubscribe('audio').then(openLiveMse))
element<HTMLSelectElement>('packaging').addEventListener('change', () => void switchPackaging())
element<HTMLSelectElement>('speed').addEventListener('change', applyPlaybackSpeed)
element<HTMLButtonElement>('liveBtn').addEventListener('click', backToLive)
element<HTMLButtonElement>('playPauseBtn').addEventListener('click', () => setPaused(!paused))
element<HTMLInputElement>('volume').addEventListener('input', applyVolume)
element<HTMLButtonElement>('fullscreenBtn').addEventListener('click', () => void toggleFullscreen())
element<HTMLButtonElement>('deliveryToggleBtn').addEventListener('click', () => {
  const overlay = element<HTMLDivElement>('delivery-overlay')
  overlay.hidden = !overlay.hidden
  element<HTMLButtonElement>('deliveryToggleBtn').setAttribute('aria-pressed', String(!overlay.hidden))
})
stage.addEventListener('fullscreenchange', renderFullscreen)
for (const type of ['pointermove', 'pointerdown', 'keydown']) {
  stage.addEventListener(type, markPointerActive)
}
for (const button of Array.from(document.querySelectorAll<HTMLButtonElement>('[data-skip-seconds]'))) {
  button.addEventListener('click', () => skip(Number(button.dataset.skipSeconds)))
}
document.addEventListener('keydown', (event) => {
  const seconds = SKIP_SECONDS_BY_KEY[event.key]
  if (seconds === undefined || usesArrowKeys(event.target)) {
    return
  }
  event.preventDefault()
  skip(seconds)
})
seekbar.addEventListener('input', () => {
  seeking = true
  renderSeekPosition(seekbar.valueAsNumber, seekbar.valueAsNumber, Number(seekbar.max))
})
seekbar.addEventListener('change', () => {
  seeking = false
  seekTo(seekbar.valueAsNumber)
})
/// The axis spans the whole broadcast but only the replayable window can be
/// fetched, so Home lands on that window instead of on a position the relay no
/// longer holds. End goes live here rather than through the browser, whose End
/// only fires `change` when it actually moves the thumb.
seekbar.addEventListener('keydown', (event) => {
  if (event.key === 'End') {
    event.preventDefault()
    seeking = false
    backToLive()
    return
  }
  if (event.key !== 'Home') {
    return
  }
  event.preventDefault()
  seeking = false
  const start = replayableStartSeconds()
  seekbar.value = String(start)
  seekTo(start)
})
for (const event of ['pointercancel', 'blur']) {
  seekbar.addEventListener(event, () => {
    seeking = false
    renderSeekbar()
  })
}
startRendering()
moqtClient.setOnSubgroupHeaderHandler((header) => streamMonitor.opened(header.trackAlias, header.groupId))
element<HTMLInputElement>('stream-gops').addEventListener('input', (event) => {
  const keptGroups = Number((event.target as HTMLInputElement).value) || 1
  streamMonitor.setKeptGroups(keptGroups)
  mp4Publisher.sentStreams.setKeptGroups(keptGroups)
})
element<HTMLInputElement>('playout-buffer').addEventListener('change', (event) => {
  const delayMs = Number((event.target as HTMLInputElement).value)
  livePlayout.setPlayoutDelayMs(Number.isFinite(delayMs) && delayMs >= 0 ? delayMs : DEFAULT_PLAYOUT_DELAY_MS)
  applyDecoderConfig()
  appendLog('info', `playout buffer ${livePlayout.playoutDelayMs()} ms`)
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
    watching = true
    setStatus('connection-status', `Connected: ${url}`, 'ok')
    appendLog('info', `connected to ${url}`)
    await subscribeCatalog()
  } catch (error) {
    setStatus('connection-status', `Failed: ${getErrorMessage(error)}`, 'error')
    appendLog('error', getErrorMessage(error))
  }
}

async function stopStream(): Promise<void> {
  watching = false
  viewerDelayMs = undefined
  livePictureSink.detach()
  timeline.reset()
  streamMonitor.reset()
  decodedFrameIds.clear()
  mediaTimeline.reset()
  mediaTimelineTrackName = undefined
  mediaTimelineDepends = []
  newestAudioGroupId = undefined
  backToLive()
  closeMse()
  livePlayout.reset()
  bufferingSpinner.hide()
  showPicture(livePictureSink.element)
  for (const kind of subscriptions.keys()) {
    await unsubscribeTrack(kind)
  }
  catalogGroupId = undefined
  videoTracks = []
  audioTracks = []
  cmafTracks = []
  renderTrackOptions()
  videoObjectCount = 0
  if (moqtClient.getConnectionStatus()) {
    await moqtClient.disconnect()
  }
  setStatus('connection-status', 'Not connected', 'idle')
  setStatus('catalog-status', 'Catalog not loaded yet', 'idle')
  setStatus('playback-status', 'Playback idle', 'idle')
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

/// A SUBSCRIBE delivers objects published after the largest one and the bridge
/// publishes the catalog once per upstream subscription, so a viewer joining a
/// subscription the relay already holds would never see it; the group
/// SUBSCRIBE_OK names as the largest is fetched as well. The FETCH and the
/// SUBSCRIBE race, and the relay keeps the catalog of a publisher that has
/// since been replaced, so the catalog of the newest group wins whatever order
/// they arrive in.
async function subscribeCatalog(): Promise<void> {
  const onText = (text: string, groupId: bigint) => {
    if (catalogGroupId !== undefined && groupId < catalogGroupId) {
      return
    }
    catalogGroupId = groupId
    void applyCatalog(text)
  }
  const subscribeOk = await subscribeTextTrack(MEDIA_CATALOG_TRACK_NAME, onText)
  await fetchLatestText(MEDIA_CATALOG_TRACK_NAME, subscribeOk, onText)
}

type TextTrackHandler = (text: string, groupId: bigint) => void

async function subscribeTextTrack(name: string, onText: TextTrackHandler): Promise<SubscribeOk> {
  const namespace = trackNamespace()
  const { subscribeOk } = await moqtClient.subscribe(namespace, name, AUTH_INFO, { forward: true })
  moqtClient.setOnSubgroupObjectHandler(
    subscribeOk.trackAlias,
    monitored(subscribeOk.trackAlias, name, (groupId, object) => {
      const payload = new Uint8Array(object.objectPayload)
      if (payload.byteLength > 0) {
        onText(new TextDecoder().decode(payload), groupId)
      }
    })
  )
  appendLog('info', `subscribed ${namespace.join('/')}/${name}`)
  return subscribeOk
}

/// draft-ietf-moq-transport-14 §9.8: SUBSCRIBE_OK names a Largest Location
/// only when content exists; without one nothing has been published yet, so
/// there is nothing to fetch and the first object arrives on the SUBSCRIBE.
async function fetchLatestText(name: string, subscribeOk: SubscribeOk, onText: TextTrackHandler): Promise<void> {
  const largestGroup = subscribeOk.largestGroupId
  if (largestGroup === undefined) {
    appendLog('info', `${name} has no published object yet; waiting for it on the subscription`)
    return
  }
  const endObject = (subscribeOk.largestObjectId ?? 0n) + 1n
  try {
    const { requestId } = await moqtClient.fetch(trackNamespace(), name, largestGroup, 0n, largestGroup, endObject, {
      onObject: (message) => {
        streamMonitor.fetchObject(
          message.requestId,
          name,
          message.groupId,
          message.objectId,
          message.objectPayload.byteLength
        )
        const payload = new Uint8Array(message.objectPayload)
        if (payload.byteLength > 0) {
          onText(new TextDecoder().decode(payload), message.groupId)
        }
      }
    })
    streamMonitor.fetchFinished(requestId)
    appendLog('info', `fetched ${name}`)
  } catch (error) {
    appendLog('info', `fetch ${name}: ${getErrorMessage(error)}`)
  }
}

async function applyCatalog(payload: string): Promise<void> {
  try {
    const catalog = parse_msf_catalog_json(payload)
    videoTracks = extractCatalogVideoTracks(catalog).filter(isLocTrack)
    audioTracks = extractCatalogAudioTracks(catalog).filter(isLocTrack)
    cmafTracks = extractCatalogCmafTracks(catalog)
    setStatus('catalog-status', `Catalog loaded: ${videoTracks.length} video / ${audioTracks.length} audio`, 'ok')
    const changed = renderTrackOptions()
    renderPackagingOptions()
    await subscribeMediaTimeline(catalog)
    if (changed) {
      await resubscribe('video')
      await resubscribe('audio')
      await openLiveMse()
    } else {
      reconfigureDecoders()
    }
  } catch (error) {
    setStatus('catalog-status', `Catalog error: ${getErrorMessage(error)}`, 'error')
    appendLog('error', `catalog: ${getErrorMessage(error)}`)
  }
}

async function subscribeMediaTimeline(catalog: unknown): Promise<void> {
  const [track] = extractCatalogMediaTimelineTracks(catalog)
  if (!track || mediaTimelineTrackName) {
    return
  }

  mediaTimelineTrackName = track.name
  mediaTimelineDepends = track.depends ?? []
  await subscribeTextTrack(track.name, (text) => {
    try {
      mediaTimeline.replace(text)
    } catch (error) {
      appendLog('error', `media timeline: ${getErrorMessage(error)}`)
      return
    }
    stampObservedGroups()
    renderSeekbar()
  })
}

/// The bridge lists a CMAF sibling next to every LOC track; the WebCodecs
/// decoders below only take the LOC ones.
function isLocTrack(track: MediaCatalogTrack): boolean {
  return track.packaging !== 'cmaf'
}

function cmafSibling(track: MediaCatalogTrack): MediaCatalogTrack | undefined {
  return cmafTracks.find((candidate) => candidate.name === `${track.name}${CMAF_TRACK_SUFFIX}`)
}

function renderPackagingOptions(): void {
  for (const option of Array.from(element<HTMLSelectElement>('packaging').options)) {
    option.disabled = option.value === 'cmaf' && cmafTracks.length === 0
  }
}

async function switchPackaging(): Promise<void> {
  const selected = element<HTMLSelectElement>('packaging').value as Packaging
  if (selected === packaging) {
    return
  }
  packaging = selected
  await resubscribe('video')
  await resubscribe('audio')
  if (packaging === 'cmaf') {
    await openLiveMse()
  } else {
    const previous = mse
    mse = undefined
    replacePicture(livePictureSink.element, () => packaging === 'loc' && !reviewing, previous)
  }
  appendLog('info', `packaging switched to ${packaging}`)
}

/// A group observed on a CMAF track, whose objects carry no LOC header, or
/// reported by TRACK_STATUS is stamped with the encode wallclock the media
/// timeline records for it. Only observed groups enter the timeline: the relay
/// caches a track from its first subscriber on, so earlier groups the media
/// timeline lists cannot be fetched.
function observeTimelineGroup(groupId: bigint): void {
  unstampedGroups.add(groupId)
  stampObservedGroups()
}

function stampObservedGroups(): void {
  for (const groupId of unstampedGroups) {
    const encodedAtMs = mediaTimeline.encodedAtMsFor(groupId)
    if (encodedAtMs !== undefined) {
      timeline.recordCapture(groupId, encodedAtMs * 1_000)
      unstampedGroups.delete(groupId)
    }
  }
}

function cmafSource(track: MediaCatalogTrack): MseTrackSource | undefined {
  if (!track.initData || !track.codec) {
    return undefined
  }
  const container = track.role === 'audio' ? 'audio/mp4' : 'video/mp4'
  return { mimeType: `${container}; codecs="${track.codec}"`, initSegment: base64ToUint8Array(track.initData) }
}

function subscribedCmafSource(kind: MediaKind): MseTrackSource | undefined {
  const name = subscriptions.get(kind)?.name
  const track = cmafTracks.find((candidate) => candidate.name === name)
  return track && cmafSource(track)
}

async function openLiveMse(): Promise<void> {
  if (packaging !== 'cmaf') {
    return
  }
  const video = subscribedCmafSource('video')
  if (!video) {
    return
  }
  const previous = mse
  const next = await MseSink.open(freeMseElement(), { video, audio: subscribedCmafSource('audio') })
  mse = next
  cmafAwaitingKeyframe = true
  applyVolume()
  replacePicture(next.element, () => mse === next && !reviewing, previous)
}

function closeMse(): void {
  mse?.close()
  mse = undefined
  cmafAwaitingKeyframe = true
}

/// One picture is on screen at a time. A picture that has yet to present a
/// frame stays hidden and whatever is on screen stays until it does, so a
/// change of packaging, quality or position never shows an empty element.
function showPicture(next: HTMLElement): void {
  if (next === visiblePicture) {
    return
  }
  visiblePicture.hidden = true
  next.hidden = false
  visiblePicture = next
}

/// The sink being replaced is closed as soon as it is off screen; while it is
/// on screen it plays on until the replacement has presented a frame.
function replacePicture(next: HTMLElement, stillWanted: () => boolean, previous: MseSink | undefined): void {
  if (previous && previous.element !== visiblePicture) {
    previous.close()
  }
  const swap = () => {
    previous?.close()
    if (stillWanted()) {
      showPicture(next)
    }
  }
  if (next instanceof HTMLVideoElement) {
    next.requestVideoFrameCallback(swap)
  } else {
    requestAnimationFrame(swap)
  }
}

function livePicture(): HTMLElement {
  return mse?.element ?? livePictureSink.element
}

function freeMseElement(): HTMLVideoElement {
  const free = MSE_ELEMENT_IDS.map((id) => element<HTMLVideoElement>(id)).find((video) => !video.getAttribute('src'))
  if (!free) {
    throw new Error('every MediaSource element is in use')
  }
  return free
}

function handleCmafObject(kind: MediaKind, trackName: string, groupId: bigint, object: SubgroupObject): void {
  if (object.objectStatus != null) {
    return
  }
  if (kind === 'video') {
    videoObjectCount += 1
    if (object.objectId === 0n) {
      observeTimelineGroup(groupId)
      renderSeekbar()
    }
    if (!reviewing) {
      setStatus('playback-status', `Playing ${trackName}`, 'ok')
    }
  } else {
    newestAudioGroupId = groupId
  }
  if (!mse) {
    return
  }
  if (kind === 'video' && cmafAwaitingKeyframe) {
    if (object.objectId !== 0n) {
      return
    }
    cmafAwaitingKeyframe = false
  }
  const payload = new Uint8Array(object.objectPayload)
  if (kind === 'video') {
    mse.appendVideo(payload)
  } else {
    mse.appendAudio(payload)
  }
}

function renderTrackOptions(): boolean {
  const videoChanged = fillSelect(element<HTMLSelectElement>('video-track'), videoTracks, describeVideoTrack)
  const audioChanged = fillSelect(element<HTMLSelectElement>('audio-track'), audioTracks, (track) => track.label)
  return videoChanged || audioChanged
}

function fillSelect(
  select: HTMLSelectElement,
  tracks: MediaCatalogTrack[],
  describe: (track: MediaCatalogTrack) => string
): boolean {
  const names = tracks.map((track) => track.name)
  const current = Array.from(select.options).map((option) => option.value)
  if (names.length === current.length && names.every((name, index) => name === current[index])) {
    return false
  }

  const selected = select.value
  select.replaceChildren(
    ...tracks.map((track) => {
      const option = document.createElement('option')
      option.value = track.name
      option.textContent = describe(track)
      return option
    })
  )
  select.value = names.includes(selected) ? selected : (names[0] ?? '')
  return true
}

function describeVideoTrack(track: MediaCatalogTrack): string {
  const resolution = track.width && track.height ? ` (${track.width}x${track.height})` : ''
  return `${track.label}${resolution}`
}

async function resubscribe(kind: MediaKind): Promise<void> {
  const select = element<HTMLSelectElement>(kind === 'video' ? 'video-track' : 'audio-track')
  const trackName = select.value
  const track = (kind === 'video' ? videoTracks : audioTracks).find((candidate) => candidate.name === trackName)
  const wire = track && (packaging === 'cmaf' ? cmafSibling(track) : track)
  if (wire && subscriptions.get(kind)?.name === wire.name) {
    return
  }

  if (kind === 'video') {
    timeline.reset()
    unstampedGroups.clear()
    backToLive()
  } else {
    newestAudioGroupId = undefined
  }
  await unsubscribeTrack(kind)
  if (!track || !wire) {
    return
  }

  if (packaging === 'loc') {
    postCatalogToDecoder(kind, track)
  }
  const { requestId, subscribeOk } = await moqtClient.subscribe(trackNamespace(), wire.name, AUTH_INFO, {
    forward: true
  })
  subscriptions.set(kind, { requestId, trackAlias: subscribeOk.trackAlias, name: wire.name, track })
  if (liveForwardPaused) {
    await moqtClient.setSubscriptionForward(requestId, false)
  }
  if (packaging === 'cmaf') {
    moqtClient.setOnSubgroupObjectHandler(
      subscribeOk.trackAlias,
      monitored(subscribeOk.trackAlias, wire.name, (groupId, object) =>
        handleCmafObject(kind, wire.name, groupId, object)
      )
    )
    appendLog('info', `subscribed ${trackNamespace().join('/')}/${wire.name}`)
    return
  }
  const worker = kind === 'video' ? videoDecoderWorker : audioDecoderWorker
  moqtClient.setOnSubgroupObjectHandler(
    subscribeOk.trackAlias,
    monitored(subscribeOk.trackAlias, wire.name, (groupId, object) => {
      if (kind === 'video') {
        videoObjectCount += 1
        timeline.record(groupId, object.locHeader, monotonicUnixMicros())
        renderSeekbar()
        if (!reviewing) {
          setStatus('playback-status', `Playing ${trackName}`, 'ok')
        }
      } else {
        newestAudioGroupId = groupId
      }
      postSubgroupObjectToWorker(worker, groupId, object)
    })
  )
  appendLog('info', `subscribed ${trackNamespace().join('/')}/${trackName}`)
}

async function unsubscribeTrack(kind: MediaKind): Promise<void> {
  const subscription = subscriptions.get(kind)
  if (!subscription) {
    return
  }

  subscriptions.delete(kind)
  moqtClient.clearSubgroupObjectHandler(subscription.trackAlias)
  if (moqtClient.getConnectionStatus()) {
    await moqtClient.unsubscribe(subscription.requestId)
  }
  appendLog('info', `unsubscribed ${subscription.name}`)
}

/// A catalog update may redefine a track under the same name, as when the
/// publisher is replaced by one with another audio codec, so the decoders
/// take the new definition of the tracks they are already subscribed to.
function reconfigureDecoders(): void {
  if (packaging !== 'loc') {
    return
  }
  for (const [kind, subscription] of subscriptions) {
    const track = (kind === 'video' ? videoTracks : audioTracks).find(
      (candidate) => candidate.name === subscription.track.name
    )
    if (!track || JSON.stringify(track) === JSON.stringify(subscription.track)) {
      continue
    }
    postCatalogToDecoder(kind, track)
    subscriptions.set(kind, { ...subscription, track })
    appendLog('info', `${kind} track ${track.name} redefined by the catalog`)
  }
}

function postCatalogToDecoder(kind: MediaKind, track: MediaCatalogTrack): void {
  if (kind === 'video') {
    postVideoCatalogToWorker(videoDecoderWorker, {
      codec: track.codec,
      initData: track.initData,
      avcFormat: track.initData ? undefined : ANNEX_B_FORMAT
    })
    return
  }
  postAudioCatalogToWorker(audioDecoderWorker, track)
}

/// The live playout paces decoded samples on one clock so that audio and
/// video stay together. All but the last `PRESENTATION_MARGIN_MS` of the
/// buffer is spent before decoding, in the video worker's jitter buffer, so
/// objects are decoded in order however they arrived and only a few decoded
/// frames are ever held.
function applyDecoderConfig(): void {
  const holdMs = Math.max(0, livePlayout.playoutDelayMs() - PRESENTATION_MARGIN_MS)
  videoDecoderWorker.postMessage({
    type: 'config',
    config: {
      telemetryEnabled: true,
      bypassJitterBuffer: holdMs === 0,
      holdMs,
      releaseMarginMs: PRESENTATION_MARGIN_MS,
      pacing: { preset: 'disabled' }
    }
  })
  audioDecoderWorker.postMessage({ type: 'config', config: { telemetryEnabled: true, bypassJitterBuffer: true } })
}

function showLiveFrame(frame: VideoFrame): void {
  viewerDelayMs = frame.timestamp ? (monotonicUnixMicros() - frame.timestamp) / 1_000 : undefined
  updateVideoStats(frame)
  markPlayhead(frame)
  livePictureSink.present(frame)
}

function showReviewFrame(frame: VideoFrame): void {
  const ids = reviewFrameIds.get(frame.timestamp)
  reviewFrameIds.delete(frame.timestamp)
  if (ids) {
    streamMonitor.setPlayhead({ ...ids, captureMicros: frame.timestamp })
  }
  const canvas = element<HTMLCanvasElement>('review')
  const context = canvas.getContext('2d')
  if (context) {
    canvas.width = frame.displayWidth
    canvas.height = frame.displayHeight
    context.drawImage(frame, 0, 0)
    showPicture(canvas)
    notePresentedFrame(canvas)
    advanceReviewPlayhead(frame.timestamp)
  }
  frame.close()
}

function startRendering(): void {
  applyDecoderConfig()
  for (const id of ['video', ...MSE_ELEMENT_IDS]) {
    watchPresentedFrames(element<HTMLVideoElement>(id))
  }

  videoDecoderWorker.onmessage = (event) => {
    if (event.data.type === 'bitrate') {
      receivedKbps = event.data.kbps ?? receivedKbps
      return
    }
    if (event.data.type === 'frame') {
      const frame = event.data.frame as VideoFrame
      decodedFrameIds.set(frame.timestamp, { groupId: event.data.groupId, objectId: event.data.objectId })
      livePlayout.presentVideo(frame)
    }
  }

  audioDecoderWorker.onmessage = (event) => {
    if (event.data.type === 'audioData') {
      livePlayout.playAudio(event.data.audioData as AudioData, event.data.captureTimestampMicros as number | undefined)
    }
  }
}

function watchPresentedFrames(video: HTMLVideoElement): void {
  const onFrame = () => {
    notePresentedFrame(video)
    video.requestVideoFrameCallback(onFrame)
  }
  video.requestVideoFrameCallback(onFrame)
}

/// A picture being replaced plays on until its successor has presented a
/// frame, and the live picture plays out what it had buffered behind a review;
/// only the picture playback is trying to show counts as progress.
function notePresentedFrame(picture: HTMLElement): void {
  if (paused || picture !== wantedPicture()) {
    return
  }
  bufferingSpinner.framePresented()
}

function wantedPicture(): HTMLElement | undefined {
  if (!reviewing) {
    return livePicture()
  }
  return packaging === 'cmaf' ? reviewMse?.element : element<HTMLCanvasElement>('review')
}

function updateVideoStats(frame: VideoFrame): void {
  const stats = element<HTMLSpanElement>('video-stats')
  const delay = viewerDelayMs === undefined ? '' : ` · delay ${Math.round(viewerDelayMs)} ms`
  stats.textContent = `${frame.displayWidth}x${frame.displayHeight}${delay} · ${Math.round(receivedKbps)} kbps · ${videoObjectCount} objects · A/V ${formatSyncOffset(livePlayout.syncOffsetMs())} · audio breaks ${livePlayout.audioBreaks()} · video ${livePlayout.videoDrops()} · re-anchors ${livePlayout.reanchors}`
}

function formatSyncOffset(offsetMs: number | undefined): string {
  if (offsetMs === undefined) {
    return '--'
  }
  const rounded = Math.round(offsetMs)
  return `${rounded < 0 ? '-' : '+'}${Math.abs(rounded)} ms`
}

function markPlayhead(frame: VideoFrame): void {
  const ids = decodedFrameIds.get(frame.timestamp)
  decodedFrameIds.delete(frame.timestamp)
  const trackAlias = subscriptions.get('video')?.trackAlias
  if (ids && trackAlias !== undefined) {
    streamMonitor.setPlayhead({ kind: 'subscribe', trackAlias, ...ids, captureMicros: frame.timestamp })
  }
}

function monitored(
  trackAlias: bigint,
  track: string,
  handler: (groupId: bigint, object: SubgroupObject) => void
): (groupId: bigint, object: SubgroupObject) => void {
  streamMonitor.label(trackAlias, track)
  return (groupId, object) => {
    streamMonitor.object(
      trackAlias,
      groupId,
      object.objectId,
      object.objectPayloadLength,
      object.objectStatus != null,
      Date.now(),
      readLocHeader(object.locHeader).captureTimestampMicros
    )
    handler(groupId, object)
  }
}

function renderPublishStreams(): void {
  element<HTMLElement>('publish-live').style.display = mp4Publisher.publishing ? '' : 'none'
  element<HTMLSpanElement>('publish-latency').textContent =
    watching && packaging === 'loc' && !reviewing && viewerDelayMs !== undefined
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
  renderPublishStreams()
  const reviewGrid = element<SVGSVGElement>('delivery-grid-review')
  const reviewTimeline = element<SVGSVGElement>('stream-monitor-review')
  if (!watching) {
    renderIdleStreamMonitor(element<SVGSVGElement>('stream-monitor'))
    renderIdleStreamMonitor(element<SVGSVGElement>('delivery-grid'))
    reviewGrid.style.display = 'none'
    reviewTimeline.style.display = 'none'
    element<HTMLSpanElement>('stream-stats').textContent = '-'
    return
  }
  const now = Date.now()
  const records = streamMonitor.snapshot()
  const liveRecords = records.filter((record) => record.kind === 'subscribe')
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
  renderDeliveryGrid(
    element<SVGSVGElement>('delivery-grid'),
    records,
    [
      { label: 'audio', kind: 'subscribe', trackAlias: subscriptions.get('audio')?.trackAlias },
      { label: 'video', kind: 'subscribe', trackAlias: subscriptions.get('video')?.trackAlias }
    ],
    streamMonitor.slotsPerTrack(),
    livePlayhead,
    livePlayout.playoutDelayMs()
  )
  reviewGrid.style.display = reviewing ? '' : 'none'
  if (reviewing) {
    renderDeliveryGrid(
      reviewGrid,
      records,
      [
        {
          label: 'fetch audio',
          kind: 'fetch',
          trackAlias: reviewFetchIds.audio,
          cadenceAlias: subscriptions.get('audio')?.trackAlias
        },
        {
          label: 'fetch video',
          kind: 'fetch',
          trackAlias: reviewFetchIds.video,
          cadenceAlias: subscriptions.get('video')?.trackAlias
        }
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

/// Selects and text inputs use the arrow keys themselves. The seek bar's own
/// stepping is replaced so that the vertical arrows move by five seconds.
function usesArrowKeys(target: EventTarget | null): boolean {
  return target instanceof HTMLSelectElement || (target instanceof HTMLInputElement && target !== seekbar)
}

function skip(seconds: number): void {
  const latest = timeline.latest
  if (!latest) {
    return
  }
  seekToCapture((reviewPlayheadMicros ?? latest.captureMicros) + seconds * MICROS_PER_SECOND)
}

/// A position at or past the live edge goes live. Any other position is
/// replayed from the closed keyframe group that holds it: the frames before it
/// are decoded without pacing and only the ones from the position on are shown.
function seekToCapture(captureMicros: number): void {
  const latest = timeline.latest
  if (!latest) {
    return
  }
  if (captureMicros >= latest.captureMicros) {
    backToLive()
    return
  }
  const target = timeline.resolveSeekTarget(captureMicros)
  if (!target) {
    setStatus('rewind-status', 'Rewind unavailable: nothing buffered yet', 'error')
    return
  }

  const generation = ++reviewGeneration
  reviewing = true
  pauseLiveForward()
  void followLiveEdge(generation)
  reviewMseOpened = false
  setPaused(false)
  reviewOriginMicros = target.captureMicros
  reviewAnchorMicros = Math.max(captureMicros, target.captureMicros)
  reviewPlayheadMicros = reviewAnchorMicros
  reviewPlayout.start(reviewAnchorMicros)
  applyVolume()
  renderSeekbar()
  setStatus('playback-status', 'Reviewing', 'review')
  void review(target.groupId, generation)
}

/// Review playback is paced by capture timestamps, so it trails the live edge
/// until the viewer asks to go back. The FETCH for the next window is issued
/// while the current one plays, so a window boundary does not stall on the
/// request.
async function review(startGroup: bigint, generation: number): Promise<void> {
  let pending = await fetchReviewWindow(startGroup, generation)
  while (pending && generation === reviewGeneration) {
    const upcoming = fetchReviewWindow(pending.nextGroup, generation)
    reviewBehindSeconds = timeline.secondsBehindLive(pending.start)
    renderReviewStatus()
    const frames = sortReviewFrames(pending.frames)
    const played =
      packaging === 'cmaf'
        ? await playReviewMse(frames, pending.audio, generation)
        : await playReview(frames, pending.audio, generation)
    pending = played ? await upcoming : undefined
  }
}

/// Review plays what FETCH brings, so the live subscriptions stop forwarding
/// for its duration and deliver again, from the next group, on the way back.
function pauseLiveForward(): void {
  if (liveForwardPaused) {
    return
  }
  liveForwardPaused = true
  updateLiveForward(false)
}

function resumeLiveForward(): void {
  if (!liveForwardPaused) {
    return
  }
  liveForwardPaused = false
  mse?.resumeAtNewestRange()
  updateLiveForward(true)
}

/// A pause and a resume in quick succession must reach every subscription in
/// that order, so the updates are chained rather than sent concurrently.
function updateLiveForward(forward: boolean): void {
  liveForwardUpdate = liveForwardUpdate.then(() => setLiveForward(forward))
}

async function setLiveForward(forward: boolean): Promise<void> {
  if (!moqtClient.getConnectionStatus()) {
    return
  }
  for (const subscription of subscriptions.values()) {
    try {
      await moqtClient.setSubscriptionForward(subscription.requestId, forward)
    } catch (error) {
      appendLog('error', `forward ${subscription.name}: ${getErrorMessage(error)}`)
    }
  }
  appendLog('info', `live subscriptions ${forward ? 'resumed' : 'paused'}`)
}

/// Without live delivery the timeline would stop at the review's start, so
/// TRACK_STATUS stands in for it: the groups it reports as the largest are the
/// ones review may fetch up to next. A relay that cannot answer gets the live
/// subscriptions forwarding again.
async function followLiveEdge(generation: number): Promise<void> {
  while (generation === reviewGeneration && liveForwardPaused) {
    try {
      const [videoGroup, audioGroup] = await Promise.all([largestLiveGroup('video'), largestLiveGroup('audio')])
      if (generation !== reviewGeneration) {
        return
      }
      if (videoGroup !== undefined) {
        observeLiveVideoGroup(videoGroup)
      }
      if (audioGroup !== undefined) {
        newestAudioGroupId = audioGroup
      }
    } catch (error) {
      appendLog('warn', `track status: ${getErrorMessage(error)}; live subscriptions forward during review`)
      resumeLiveForward()
      return
    }
    await new Promise((resolve) => setTimeout(resolve, TRACK_STATUS_POLL_MS))
  }
}

async function largestLiveGroup(kind: MediaKind): Promise<bigint | undefined> {
  const name = subscriptions.get(kind)?.name
  if (!name) {
    return undefined
  }
  const status = await moqtClient.trackStatus(trackNamespace(), name, '')
  return status.contentExists ? status.largestGroupId : undefined
}

/// The media timeline records the groups of the tracks it depends on. A group
/// of any other rendition is placed from the arrival lag of the last live
/// object, which runs late by however long the main thread took to handle the
/// TRACK_STATUS answer.
function observeLiveVideoGroup(groupId: bigint): void {
  const name = subscriptions.get('video')?.name
  if (packaging === 'cmaf' || (name !== undefined && mediaTimelineDepends.includes(name))) {
    observeTimelineGroup(groupId)
  } else {
    timeline.recordLiveGroup(groupId, monotonicUnixMicros())
  }
  renderSeekbar()
}

/// The bridge starts the audio groups at the video keyframes with the same
/// ids, so the audio of a window is the same group range on the audio track
/// and is fetched alongside the video.
async function fetchReviewWindow(start: bigint, generation: number): Promise<ReviewWindow | undefined> {
  const end = await awaitClosedWindowEnd(start, generation)
  const subscription = subscriptions.get('video')
  if (end === undefined || !subscription) {
    return undefined
  }
  const audioName = subscriptions.get('audio')?.name
  const [frames, audio] = await Promise.all([
    fetchFrames(subscription.name, start, end, generation),
    audioName ? fetchReviewAudio(audioName, start, end, generation) : Promise.resolve([])
  ])
  if (!frames) {
    return undefined
  }
  if (frames.length === 0) {
    setStatus('rewind-status', 'Rewind unavailable: no cached objects', 'error')
    return undefined
  }
  appendLog('info', `fetched ${frames.length} objects from group ${start}`)
  reviewFetchIds = { video: frames[0]?.requestId, audio: audio?.[0]?.requestId }
  return { start, nextGroup: end + 1n, frames, audio: sortReviewFrames(audio ?? []) }
}

/// The audio of a group ends a little after its video: the source interleaves
/// audio behind video, so audio captured just before a keyframe arrives after
/// that keyframe has opened the next group. The audio group is fetched once
/// the audio track has moved on to a later group, or after a bounded wait.
async function fetchReviewAudio(
  trackName: string,
  start: bigint,
  end: bigint,
  generation: number
): Promise<ReviewFrame[] | undefined> {
  const deadline = performance.now() + AUDIO_GROUP_CLOSE_WAIT_MS
  while (
    generation === reviewGeneration &&
    (newestAudioGroupId === undefined || newestAudioGroupId <= end) &&
    performance.now() < deadline
  ) {
    await new Promise((resolve) => setTimeout(resolve, CLOSED_GROUP_POLL_MS))
  }
  return fetchFrames(trackName, start, end, generation)
}

async function fetchFrames(
  trackName: string,
  start: bigint,
  end: bigint,
  generation: number
): Promise<ReviewFrame[] | undefined> {
  const frames: ReviewFrame[] = []
  let requestId: bigint | undefined
  let endStream: (end: FetchStreamEnd) => void = () => {}
  const streamEnd = new Promise<FetchStreamEnd>((resolve) => {
    endStream = resolve
  })
  try {
    ;({ requestId } = await moqtClient.fetch(trackNamespace(), trackName, start, 0n, end, 0n, {
      onObject: (message) => {
        const frame = toReviewFrame(message)
        streamMonitor.fetchObject(
          message.requestId,
          trackName,
          message.groupId,
          message.objectId,
          message.objectPayload.byteLength,
          frame?.captureMicros
        )
        if (generation !== reviewGeneration) {
          return
        }
        if (frame) {
          frame.requestId = message.requestId
          frames.push(frame)
        }
      },
      onStreamEnd: (message) =>
        endStream(message.isReset ? { kind: 'reset', code: message.resetErrorCode } : { kind: 'fin' })
    }))
  } catch (error) {
    if (generation === reviewGeneration) {
      setStatus('rewind-status', `Rewind failed: ${getErrorMessage(error)}`, 'error')
      appendLog('error', `fetch ${trackName}: ${getErrorMessage(error)}`)
    }
    return undefined
  }

  const outcome = await waitForFetchStreamEnd(streamEnd)
  if (requestId !== undefined) {
    streamMonitor.fetchFinished(requestId)
  }
  if (generation !== reviewGeneration) {
    return undefined
  }
  if (outcome.kind === 'reset') {
    setStatus('rewind-status', `Rewind failed: fetch stream reset (code ${outcome.code ?? 'unknown'})`, 'error')
    appendLog('error', `fetch ${trackName}: stream reset (code ${outcome.code ?? 'unknown'})`)
    return undefined
  }
  if (outcome.kind === 'deadline') {
    appendLog(
      'warn',
      `fetch ${trackName}: no stream end within ${FETCH_DEADLINE_MS} ms, playing ${frames.length} objects`
    )
  }
  return frames
}

/// The live edge group is still open and a FETCH that reaches into it escapes
/// the relay cache, so a window can only end at the newest closed group. Once
/// playback has consumed those, wait for the publisher to close another one;
/// at more than real time that wait would recur on every group from then on,
/// so review that has played out everything fetched goes live instead.
async function awaitClosedWindowEnd(start: bigint, generation: number): Promise<bigint | undefined> {
  while (generation === reviewGeneration) {
    const newestClosed = timeline.newestClosed
    if (newestClosed && start <= newestClosed.groupId) {
      const bounded = start + REWIND_GROUP_COUNT
      return bounded < newestClosed.groupId ? bounded : newestClosed.groupId
    }
    if (newestClosed && playbackSpeed() > 1 && reviewBufferDrained()) {
      backToLive()
      return undefined
    }
    await new Promise((resolve) => setTimeout(resolve, CLOSED_GROUP_POLL_MS))
  }
  return undefined
}

type FetchStreamEnd = { kind: 'fin' } | { kind: 'reset'; code: bigint | undefined } | { kind: 'deadline' }

/// The relay FINs the fetch stream once every object up to the FETCH_OK End
/// Location is written; the deadline only guards against a stream that never
/// ends.
async function waitForFetchStreamEnd(streamEnd: Promise<FetchStreamEnd>): Promise<FetchStreamEnd> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const deadline = new Promise<FetchStreamEnd>((resolve) => {
    timer = setTimeout(() => resolve({ kind: 'deadline' }), FETCH_DEADLINE_MS)
  })
  try {
    return await Promise.race([streamEnd, deadline])
  } finally {
    clearTimeout(timer)
  }
}

/// The window's audio is decoded up front. Frames are decoded a little ahead
/// of their presentation, not the whole window at once: decoded frames hold
/// GPU memory until they are shown. The function returns shortly before the
/// last frame is due so the next window is decoded in time to follow on.
async function playReview(frames: ReviewFrame[], audio: ReviewFrame[], generation: number): Promise<boolean> {
  const config = pendingReviewConfig()
  if (!config) {
    setStatus('rewind-status', 'Rewind unavailable: the video track has no codec', 'error')
    return false
  }
  const audioConfig = reviewAudioConfig()
  if (audioConfig) {
    reviewPlayout.decodeAudio(audio, audioConfig)
  }
  const decoder = new VideoDecoder({
    output: (frame) => {
      if (generation !== reviewGeneration) {
        frame.close()
        return
      }
      reviewPlayout.presentVideo(frame)
    },
    error: (error) => appendLog('error', `review decoder: ${error.message}`)
  })
  decoder.configure(config)

  const origin = frames[0].captureMicros ?? reviewOriginMicros ?? 0
  for (const frame of frames) {
    while (
      generation === reviewGeneration &&
      reviewPlayout.queuedVideo + decoder.decodeQueueSize > REVIEW_VIDEO_AHEAD_FRAMES
    ) {
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
    if (generation !== reviewGeneration || decoder.state === 'closed') {
      break
    }
    if (frame.captureMicros !== undefined && frame.requestId !== undefined) {
      reviewFrameIds.set(frame.captureMicros, {
        kind: 'fetch',
        trackAlias: frame.requestId,
        groupId: frame.groupId,
        objectId: frame.objectId
      })
    }
    decoder.decode(
      new EncodedVideoChunk({
        type: frame.objectId === 0n ? 'key' : 'delta',
        timestamp: frame.captureMicros ?? origin,
        data: frame.data
      })
    )
  }
  if (decoder.state !== 'closed') {
    await decoder.flush().catch(() => undefined)
    decoder.close()
  }
  const last = frames[frames.length - 1]?.captureMicros
  if (last !== undefined) {
    await reviewPlayout.waitUntilDue(last - REVIEW_HANDOVER_MICROS, () => generation === reviewGeneration)
  }
  return generation === reviewGeneration
}

/// Fetched fragments are appended to a MediaSource on its own element; the live
/// one stays open hidden and continues from the newest range it is given once
/// live delivery resumes. The next window is fetched once playback has caught
/// up to within a few seconds of what is buffered.
async function playReviewMse(frames: ReviewFrame[], audio: ReviewFrame[], generation: number): Promise<boolean> {
  if (!reviewMseOpened) {
    const source = subscribedCmafSource('video')
    if (!source) {
      setStatus('rewind-status', 'Rewind unavailable: the CMAF track has no init segment', 'error')
      return false
    }
    const origin = reviewOriginMicros ?? 0
    const startAtSeconds = ((reviewAnchorMicros ?? origin) - origin) / MICROS_PER_SECOND
    const previous = reviewMse
    const next = await MseSink.open(freeMseElement(), {
      video: source,
      audio: subscribedCmafSource('audio'),
      startAtSeconds
    })
    reviewMse = next
    reviewMseOpened = true
    applyVolume()
    applyPlaybackSpeed()
    replacePicture(next.element, () => generation === reviewGeneration, previous)
    next.element.addEventListener('timeupdate', () => {
      if (generation === reviewGeneration) {
        advanceReviewPlayhead(origin + next.secondsFromStart() * MICROS_PER_SECOND)
      }
    })
  }
  const sink = reviewMse
  if (generation !== reviewGeneration || !sink) {
    return false
  }
  for (const frame of frames) {
    sink.appendVideo(frame.data)
  }
  for (const chunk of audio) {
    sink.appendAudio(chunk.data)
  }
  while (generation === reviewGeneration) {
    const ahead = (sink.bufferedEnd() ?? 0) - sink.element.currentTime
    if (ahead < REVIEW_BUFFER_AHEAD_SECONDS) {
      break
    }
    await new Promise((resolve) => setTimeout(resolve, CLOSED_GROUP_POLL_MS))
  }
  return generation === reviewGeneration
}

function closeReviewMse(): void {
  reviewMse?.close()
  reviewMse = undefined
  reviewMseOpened = false
}

function pendingReviewConfig(): VideoDecoderConfig | undefined {
  const track = videoTracks.find((candidate) => candidate.name === subscriptions.get('video')?.name)
  if (!track?.codec) {
    return undefined
  }
  return { codec: track.codec, optimizeForLatency: true }
}

function reviewAudioConfig(): AudioDecoderConfig | undefined {
  const track = audioTracks.find((candidate) => candidate.name === subscriptions.get('audio')?.name)
  if (!track?.codec || !track.samplerate) {
    return undefined
  }
  return {
    codec: track.codec,
    sampleRate: track.samplerate,
    numberOfChannels: parseAudioChannelCount(track.channelConfig) ?? 2,
    description: track.initData ? base64ToUint8Array(track.initData) : undefined
  }
}

function backToLive(): void {
  reviewGeneration += 1
  reviewing = false
  resumeLiveForward()
  reviewFrameIds.clear()
  reviewFetchIds = {}
  streamMonitor.clearPlayhead('fetch')
  seeking = false
  setPaused(false)
  reviewAnchorMicros = undefined
  reviewOriginMicros = undefined
  reviewPlayheadMicros = undefined
  reviewPlayout.stop()
  element<HTMLSelectElement>('speed').value = '1'
  applyVolume()
  renderSeekbar()
  showPicture(livePicture())
  closeReviewMse()
  setStatus('rewind-status', 'Live', 'ok')
}

/// Pausing holds whatever is on screen; every other transition (seek, skip,
/// live, packaging or quality change) resumes. Resuming live CMAF jumps to the
/// end of what is buffered so the picture is live again; the LOC MediaStream
/// has no backlog to skip.
function setPaused(next: boolean): void {
  paused = next
  const button = element<HTMLButtonElement>('playPauseBtn')
  button.textContent = paused ? '\u25B6' : '\u275A\u275A'
  button.setAttribute('aria-label', paused ? 'Play' : 'Pause')
  button.setAttribute('aria-pressed', String(paused))
  if (paused) {
    bufferingSpinner.hide()
  }
  for (const media of playingMedia()) {
    if (paused) {
      media.pause()
    } else {
      void media.play().catch(() => undefined)
    }
  }
  if (!reviewing && !mse) {
    livePlayout.setPaused(paused)
  }
  if (reviewing && packaging === 'loc') {
    reviewPlayout.setPaused(paused)
  }
  const liveEnd = mse?.bufferedEnd()
  if (!paused && !reviewing && mse && liveEnd !== undefined) {
    mse.element.currentTime = liveEnd
  }
}

async function toggleFullscreen(): Promise<void> {
  try {
    if (document.fullscreenElement === stage) {
      await document.exitFullscreen()
    } else {
      await stage.requestFullscreen()
    }
  } catch (error) {
    appendLog('error', `fullscreen: ${getErrorMessage(error)}`)
  }
}

function renderFullscreen(): void {
  const fullscreen = document.fullscreenElement === stage
  const button = element<HTMLButtonElement>('fullscreenBtn')
  button.setAttribute('aria-label', fullscreen ? 'Exit fullscreen' : 'Fullscreen')
  button.title = fullscreen ? '全画面を終了' : '全画面'
  markPointerActive()
}

function markPointerActive(): void {
  stage.classList.remove('pointer-idle')
  if (pointerIdleTimer !== undefined) {
    clearTimeout(pointerIdleTimer)
  }
  pointerIdleTimer = setTimeout(() => {
    pointerIdleTimer = undefined
    stage.classList.add('pointer-idle')
  }, POINTER_IDLE_MS)
}

function playingMedia(): HTMLMediaElement[] {
  if (reviewing) {
    return reviewMse ? [reviewMse.element] : []
  }
  if (mse) {
    return [mse.element]
  }
  return livePictureSink.element instanceof HTMLMediaElement ? [livePictureSink.element] : []
}

/// Review carries its own sound, so whatever live audio is still buffered when
/// a review starts is silenced, and it is heard again on the way back to live.
function applyVolume(): void {
  volume = element<HTMLInputElement>('volume').valueAsNumber
  livePlayout.setVolume(reviewing ? 0 : volume)
  reviewPlayout.setVolume(volume)
  if (mse) {
    mse.element.volume = reviewing ? 0 : volume
  }
  if (reviewMse) {
    reviewMse.element.volume = volume
  }
}

/// Only review playback through MSE can run at another rate: live playback
/// has to keep pace with the publisher, and the WebCodecs path paces frames
/// itself. Loading a new source resets the element's rate, so the chosen speed
/// is applied again whenever the review MediaSource is opened.
function speedAdjustable(): boolean {
  return packaging === 'cmaf' && reviewing
}

function reviewBufferDrained(): boolean {
  return (
    reviewMse !== undefined && (reviewMse.bufferedEnd() ?? 0) - reviewMse.element.currentTime < REVIEW_DRAINED_SECONDS
  )
}

function playbackSpeed(): number {
  return speedAdjustable() ? Number(element<HTMLSelectElement>('speed').value) : 1
}

function applyPlaybackSpeed(): void {
  element<HTMLSelectElement>('speed').disabled = !speedAdjustable()
  if (speedAdjustable() && reviewMse) {
    reviewMse.element.playbackRate = playbackSpeed()
  }
}

/// The axis runs from the start of the broadcast, which the media timeline
/// places, so the bar keeps its meaning as cache retention grows. Until the
/// first timeline object arrives it falls back to the replayable window.
function renderSeekbar(): void {
  element<HTMLDivElement>('seek-available-window').dataset.seconds = timeline.span.toFixed(1)
  element<HTMLButtonElement>('liveBtn').classList.toggle('reviewing', reviewing)
  applyPlaybackSpeed()
  if (seeking) {
    return
  }
  const latest = liveEdgeSeconds()
  const replayableStart = replayableStartSeconds()
  const broadcastStart = mediaTimeline.broadcastStartMicros()
  seekbar.min = String(broadcastStart === undefined ? replayableStart : broadcastStart / 1_000_000)
  seekbar.max = String(latest)
  seekbar.disabled = !timeline.newestClosed || timeline.span <= 0
  const anchor = reviewAnchorMicros === undefined ? latest : reviewAnchorMicros / 1_000_000
  const playhead = reviewPlayheadMicros === undefined ? latest : reviewPlayheadMicros / 1_000_000
  seekbar.valueAsNumber = anchor
  renderReplayableWindow(replayableStart, latest)
  renderReviewProgress(anchor, playhead, latest)
  renderSeekPosition(anchor, playhead, latest)
}

/// The decoder emits frames in bursts, so the readout steps a second at a time
/// instead of following every frame. The thumb stays on the position that was
/// seeked to and the progress fill carries the movement.
function advanceReviewPlayhead(captureMicros: number): void {
  if (reviewPlayheadMicros !== undefined && Math.abs(captureMicros - reviewPlayheadMicros) < REVIEW_PLAYHEAD_STEP_US) {
    return
  }
  reviewPlayheadMicros = captureMicros
  renderReviewStatus()
  renderSeekbar()
}

/// Starting the next window's fetch can already have taken playback live, so
/// a status for the window is only shown while still reviewing.
function renderReviewStatus(): void {
  if (!reviewing) {
    return
  }
  const offset = reviewPlayout.syncOffsetMs()
  const sync = offset === undefined ? '' : ` · A/V ${formatSyncOffset(offset)}`
  setStatus('rewind-status', `Rewound ${reviewBehindSeconds.toFixed(1)}s${sync}`, 'review')
}

function renderReviewProgress(anchor: number, playhead: number, latest: number): void {
  const played = element<HTMLDivElement>('seek-review-progress')
  played.hidden = !reviewing || latest <= Number(seekbar.min)
  if (played.hidden) {
    return
  }
  played.style.left = percentOfAxis(anchor - Number(seekbar.min), latest)
  played.style.width = percentOfAxis(Math.max(0, playhead - anchor), latest)
}

function renderReplayableWindow(replayableStart: number, latest: number): void {
  const min = Number(seekbar.min)
  const window = element<HTMLDivElement>('seek-available-window')
  window.style.left = percentOfAxis(replayableStart - min, latest)
  window.style.width = percentOfAxis(latest - replayableStart, latest)
  const elapsed = mediaTimeline.elapsedMsAt(min * 1_000_000)
  setStatusText('seek-start', elapsed === undefined ? '--:--' : formatElapsed(elapsed))
}

function percentOfAxis(seconds: number, latest: number): string {
  const axis = latest - Number(seekbar.min)
  return axis > 0 ? `${((seconds / axis) * 100).toFixed(3)}%` : '0%'
}

function liveEdgeSeconds(): number {
  return (timeline.latest?.captureMicros ?? 0) / MICROS_PER_SECOND
}

function replayableStartSeconds(): number {
  return liveEdgeSeconds() - timeline.span
}

/// `seekbar.max` is frozen while a drag is in progress, so comparing against it
/// rather than against the live edge keeps the right end meaning "go live" even
/// when a group arrives mid-gesture.
function seekTo(captureSeconds: number): void {
  if (captureSeconds >= Number(seekbar.max)) {
    backToLive()
    return
  }
  seekToCapture(captureSeconds * MICROS_PER_SECOND)
}

function renderSeekPosition(anchor: number, playhead: number, latest: number): void {
  const behind = Math.max(0, latest - playhead)
  const label = behind < 0.1 ? 'LIVE' : `-${behind.toFixed(1)}s`
  setStatusText('seek-position', label)
  const thumbBehind = Math.max(0, latest - anchor)
  seekbar.setAttribute('aria-valuetext', thumbBehind < 0.1 ? 'Live' : `${thumbBehind.toFixed(1)} seconds behind live`)
  renderSeekElapsed(playhead, latest)
}

function renderSeekElapsed(position: number, latest: number): void {
  const elapsed = mediaTimeline.elapsedMsAt(position * 1_000_000)
  const broadcast = mediaTimeline.elapsedMsAt(latest * 1_000_000)
  setStatusText(
    'seek-elapsed',
    elapsed === undefined || broadcast === undefined
      ? '--:-- / --:--'
      : `${formatElapsed(elapsed)} / ${formatElapsed(broadcast)}`
  )
}
