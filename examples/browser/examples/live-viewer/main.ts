import { MoqtClientWrapper } from '@moqt/moqtClient'
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
import { MseSink, type MseTrackSource } from '../../utils/media/mseSink'
import { getErrorMessage, initializeMediaExamplePage, parseTrackNamespace, setStatusText } from '../media/common'
import { LivePlayout } from './livePlayout'
import { MediaTimeline, formatElapsed } from './mediaTimeline'
import { ReviewPlayout } from './reviewPlayout'
import { GroupTimeline, type ReviewFrame, sortReviewFrames, toReviewFrame } from './rewind'

const AUTH_INFO = 'secret'
const ANNEX_B_FORMAT = 'annexb'
const TIMELINE_CAPACITY = 64
const REWIND_GROUP_COUNT = 4n
const FETCH_IDLE_MS = 400
const FETCH_DEADLINE_MS = 8_000
const CLOSED_GROUP_POLL_MS = 200
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
const FETCH_OPEN_END_GROUP = 2n ** 62n - 1n

type Packaging = 'loc' | 'cmaf'

type MediaKind = 'video' | 'audio'

type SubgroupObject = Parameters<Parameters<MoqtClientWrapper['setOnSubgroupObjectHandler']>[1]>[1]

type SubscribeOk = Awaited<ReturnType<MoqtClientWrapper['subscribe']>>['subscribeOk']

type TrackSubscription = {
  requestId: bigint
  trackAlias: bigint
  name: string
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
const videoGenerator = new MediaStreamTrackGenerator({ kind: 'video' })
const videoWriter = videoGenerator.writable.getWriter()
const livePlayout = new LivePlayout(showLiveFrame)
const reviewPlayout = new ReviewPlayout(showReviewFrame, (message) => appendLog('error', message))

let videoTracks: MediaCatalogTrack[] = []
let audioTracks: MediaCatalogTrack[] = []
let cmafTracks: MediaCatalogTrack[] = []
let packaging: Packaging = 'loc'
let mse: MseSink | undefined
let reviewMse: MseSink | undefined
let reviewMseOpened = false
let visiblePicture: HTMLElement = element('video')
/// MSE decodes from the first random access point, so after a MediaSource is
/// (re)opened live fragments are dropped until one starts a group.
let cmafAwaitingKeyframe = true
const unstampedCmafGroups = new Set<bigint>()
const subscriptions = new Map<MediaKind, TrackSubscription>()
let videoObjectCount = 0
let receivedKbps = 0
const timeline = new GroupTimeline(TIMELINE_CAPACITY)
let reviewBehindSeconds = 0
let newestAudioGroupId: bigint | undefined
const mediaTimeline = new MediaTimeline()
let mediaTimelineTrackName: string | undefined
let reviewing = false
let reviewGeneration = 0
let reviewAnchorMicros: number | undefined
let reviewOriginMicros: number | undefined
let reviewPlayheadMicros: number | undefined
let seeking = false
let paused = false
let volume = 1
const seekbar = element<HTMLInputElement>('seekbar')

initializeMediaExamplePage('namespace')
element<HTMLButtonElement>('watchBtn').addEventListener('click', () => void watchStream())
element<HTMLButtonElement>('stopBtn').addEventListener('click', () => void stopStream())
element<HTMLSelectElement>('video-track').addEventListener('change', () => void resubscribe('video').then(openLiveMse))
element<HTMLSelectElement>('audio-track').addEventListener('change', () => void resubscribe('audio').then(openLiveMse))
element<HTMLSelectElement>('packaging').addEventListener('change', () => void switchPackaging())
element<HTMLSelectElement>('speed').addEventListener('change', applyPlaybackSpeed)
element<HTMLButtonElement>('liveBtn').addEventListener('click', backToLive)
element<HTMLButtonElement>('playPauseBtn').addEventListener('click', () => setPaused(!paused))
element<HTMLInputElement>('volume').addEventListener('input', applyVolume)
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

async function watchStream(): Promise<void> {
  try {
    await stopStream()
    const url = element<HTMLInputElement>('url').value.trim()
    await moqtClient.connect(url)
    setStatusText('connection-status', `Connected: ${url}`)
    appendLog('info', `connected to ${url}`)
    await subscribeCatalog()
  } catch (error) {
    setStatusText('connection-status', `Failed: ${getErrorMessage(error)}`)
    appendLog('error', getErrorMessage(error))
  }
}

async function stopStream(): Promise<void> {
  timeline.reset()
  mediaTimeline.reset()
  mediaTimelineTrackName = undefined
  newestAudioGroupId = undefined
  backToLive()
  closeMse()
  livePlayout.reset()
  showPicture(element<HTMLVideoElement>('video'))
  for (const kind of subscriptions.keys()) {
    await unsubscribeTrack(kind)
  }
  videoTracks = []
  audioTracks = []
  cmafTracks = []
  renderTrackOptions()
  videoObjectCount = 0
  if (moqtClient.getConnectionStatus()) {
    await moqtClient.disconnect()
  }
  setStatusText('connection-status', 'Not connected')
  setStatusText('catalog-status', 'Catalog not loaded yet')
  setStatusText('playback-status', 'Playback idle')
}

/// A SUBSCRIBE delivers objects published after the largest one and the bridge
/// publishes the catalog once per upstream subscription, so a viewer joining a
/// subscription the relay already holds would never see it. The current
/// catalog is fetched instead: the group SUBSCRIBE_OK names when the relay
/// still knows it, otherwise the whole track, which the relay completes from
/// the bridge.
async function subscribeCatalog(): Promise<void> {
  const onText = (text: string) => void applyCatalog(text)
  const subscribeOk = await subscribeTextTrack(MEDIA_CATALOG_TRACK_NAME, onText)
  await fetchLatestText(MEDIA_CATALOG_TRACK_NAME, subscribeOk, onText)
}

async function subscribeTextTrack(name: string, onText: (text: string) => void): Promise<SubscribeOk> {
  const namespace = trackNamespace()
  const { subscribeOk } = await moqtClient.subscribe(namespace, name, AUTH_INFO, { forward: true })
  moqtClient.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (_groupId, object) => {
    const payload = new Uint8Array(object.objectPayload)
    if (payload.byteLength > 0) {
      onText(new TextDecoder().decode(payload))
    }
  })
  appendLog('info', `subscribed ${namespace.join('/')}/${name}`)
  return subscribeOk
}

async function fetchLatestText(name: string, subscribeOk: SubscribeOk, onText: (text: string) => void): Promise<void> {
  const largestGroup = subscribeOk.largestGroupId
  const startGroup = largestGroup ?? 0n
  const endGroup = largestGroup ?? FETCH_OPEN_END_GROUP
  const endObject = largestGroup === undefined ? 0n : (subscribeOk.largestObjectId ?? 0n) + 1n
  try {
    await moqtClient.fetch(trackNamespace(), name, startGroup, 0n, endGroup, endObject, {
      onObject: (message) => {
        const payload = new Uint8Array(message.objectPayload)
        if (payload.byteLength > 0) {
          onText(new TextDecoder().decode(payload))
        }
      }
    })
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
    setStatusText('catalog-status', `Catalog loaded: ${videoTracks.length} video / ${audioTracks.length} audio`)
    const changed = renderTrackOptions()
    renderPackagingOptions()
    await subscribeMediaTimeline(catalog)
    if (changed) {
      await resubscribe('video')
      await resubscribe('audio')
      await openLiveMse()
    }
  } catch (error) {
    setStatusText('catalog-status', `Catalog error: ${getErrorMessage(error)}`)
    appendLog('error', `catalog: ${getErrorMessage(error)}`)
  }
}

async function subscribeMediaTimeline(catalog: unknown): Promise<void> {
  const [track] = extractCatalogMediaTimelineTracks(catalog)
  if (!track || mediaTimelineTrackName) {
    return
  }

  mediaTimelineTrackName = track.name
  await subscribeTextTrack(track.name, (text) => {
    try {
      mediaTimeline.replace(text)
    } catch (error) {
      appendLog('error', `media timeline: ${getErrorMessage(error)}`)
      return
    }
    stampObservedCmafGroups()
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
    replacePicture(element<HTMLVideoElement>('video'), () => packaging === 'loc' && !reviewing, previous)
  }
  appendLog('info', `packaging switched to ${packaging}`)
}

/// CMAF objects carry no LOC header, so a group observed on the CMAF track is
/// stamped with the encode wallclock the media timeline records for it. Only
/// observed groups enter the timeline: the relay caches a track from its first
/// subscriber on, so earlier groups the media timeline lists cannot be fetched.
function observeCmafGroup(groupId: bigint): void {
  unstampedCmafGroups.add(groupId)
  stampObservedCmafGroups()
}

function stampObservedCmafGroups(): void {
  for (const groupId of unstampedCmafGroups) {
    const encodedAtMs = mediaTimeline.encodedAtMsFor(groupId)
    if (encodedAtMs !== undefined) {
      timeline.recordCapture(groupId, encodedAtMs * 1_000)
      unstampedCmafGroups.delete(groupId)
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
function replacePicture(next: HTMLVideoElement, stillWanted: () => boolean, previous: MseSink | undefined): void {
  if (previous && previous.element !== visiblePicture) {
    previous.close()
  }
  next.requestVideoFrameCallback(() => {
    previous?.close()
    if (stillWanted()) {
      showPicture(next)
    }
  })
}

function livePicture(): HTMLElement {
  return mse?.element ?? element<HTMLVideoElement>('video')
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
      observeCmafGroup(groupId)
      renderSeekbar()
    }
    if (!reviewing) {
      setStatusText('playback-status', `Playing ${trackName}`)
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
    unstampedCmafGroups.clear()
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
  subscriptions.set(kind, { requestId, trackAlias: subscribeOk.trackAlias, name: wire.name })
  if (packaging === 'cmaf') {
    moqtClient.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (groupId, object) =>
      handleCmafObject(kind, wire.name, groupId, object)
    )
    appendLog('info', `subscribed ${trackNamespace().join('/')}/${wire.name}`)
    return
  }
  const worker = kind === 'video' ? videoDecoderWorker : audioDecoderWorker
  moqtClient.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (groupId, object) => {
    const payload = new Uint8Array(object.objectPayload)
    if (kind === 'video') {
      videoObjectCount += 1
      timeline.record(groupId, object.locHeader)
      renderSeekbar()
      if (!reviewing) {
        setStatusText('playback-status', `Playing ${trackName}`)
      }
    } else {
      newestAudioGroupId = groupId
    }
    worker.postMessage(
      {
        groupId,
        subgroupStreamObject: {
          subgroupId: object.subgroupId,
          objectIdDelta: object.objectIdDelta,
          objectPayloadLength: payload.byteLength,
          objectPayload: payload,
          objectStatus: object.objectStatus,
          locHeader: object.locHeader
        }
      },
      [payload.buffer]
    )
  })
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

function postCatalogToDecoder(kind: MediaKind, track: MediaCatalogTrack): void {
  if (kind === 'video') {
    videoDecoderWorker.postMessage({
      type: 'catalog',
      codec: track.codec,
      descriptionBase64: track.initData,
      avcFormat: track.initData ? undefined : ANNEX_B_FORMAT
    })
    return
  }
  audioDecoderWorker.postMessage({
    type: 'catalog',
    codec: track.codec,
    sampleRate: track.samplerate,
    channels: channelCount(track.channelConfig),
    descriptionBase64: track.initData
  })
}

function channelCount(channelConfig?: string): number | undefined {
  if (channelConfig === 'mono') {
    return 1
  }
  if (channelConfig === 'stereo') {
    return 2
  }
  const parsed = Number.parseInt(channelConfig ?? '', 10)
  return Number.isNaN(parsed) ? undefined : parsed
}

/// The decoders hand every sample over as soon as it is decoded; the live
/// playout paces them on one clock so that audio and video stay together.
function applyDecoderConfig(): void {
  const config = { telemetryEnabled: true, bypassJitterBuffer: true }
  videoDecoderWorker.postMessage({ type: 'config', config })
  audioDecoderWorker.postMessage({ type: 'config', config })
}

function showLiveFrame(frame: VideoFrame): void {
  updateVideoStats(frame)
  if (videoWriter.desiredSize === null || videoWriter.desiredSize <= 0) {
    frame.close()
    return
  }
  void videoWriter
    .write(frame)
    .catch(() => undefined)
    .finally(() => frame.close())
}

function showReviewFrame(frame: VideoFrame): void {
  const canvas = element<HTMLCanvasElement>('review')
  const context = canvas.getContext('2d')
  if (context) {
    canvas.width = frame.displayWidth
    canvas.height = frame.displayHeight
    context.drawImage(frame, 0, 0)
    showPicture(canvas)
    advanceReviewPlayhead(frame.timestamp)
  }
  frame.close()
}

function startRendering(): void {
  applyDecoderConfig()
  element<HTMLVideoElement>('video').srcObject = new MediaStream([videoGenerator])

  videoDecoderWorker.onmessage = (event) => {
    if (event.data.type === 'bitrate') {
      receivedKbps = event.data.kbps ?? receivedKbps
      return
    }
    if (event.data.type === 'frame') {
      livePlayout.presentVideo(event.data.frame as VideoFrame)
    }
  }

  audioDecoderWorker.onmessage = (event) => {
    if (event.data.type === 'audioData') {
      livePlayout.playAudio(event.data.audioData as AudioData, event.data.captureTimestampMicros as number | undefined)
    }
  }
}

function updateVideoStats(frame: VideoFrame): void {
  const stats = element<HTMLSpanElement>('video-stats')
  stats.textContent = `${frame.displayWidth}x${frame.displayHeight} · ${Math.round(receivedKbps)} kbps · ${videoObjectCount} objects · A/V ${formatSyncOffset(livePlayout.syncOffsetMs())} · audio breaks ${livePlayout.audioBreaks()} · video ${livePlayout.videoDrops()}`
}

function formatSyncOffset(offsetMs: number | undefined): string {
  if (offsetMs === undefined) {
    return '--'
  }
  const rounded = Math.round(offsetMs)
  return `${rounded < 0 ? '-' : '+'}${Math.abs(rounded)} ms`
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

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id)
  if (!found) {
    throw new Error(`missing element: ${id}`)
  }
  return found as T
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
    setStatusText('rewind-status', 'Rewind unavailable: nothing buffered yet')
    return
  }

  const generation = ++reviewGeneration
  reviewing = true
  reviewMseOpened = false
  setPaused(false)
  reviewOriginMicros = target.captureMicros
  reviewAnchorMicros = Math.max(captureMicros, target.captureMicros)
  reviewPlayheadMicros = reviewAnchorMicros
  reviewPlayout.start(reviewAnchorMicros)
  applyVolume()
  renderSeekbar()
  setStatusText('playback-status', 'Reviewing')
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
    setStatusText('rewind-status', 'Rewind unavailable: no cached objects')
    return undefined
  }
  appendLog('info', `fetched ${frames.length} objects from group ${start}`)
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
  let lastArrival = performance.now()
  try {
    await moqtClient.fetch(trackNamespace(), trackName, start, 0n, end, 0n, {
      onObject: (message) => {
        if (generation !== reviewGeneration) {
          return
        }
        lastArrival = performance.now()
        const frame = toReviewFrame(message)
        if (frame) {
          frames.push(frame)
        }
      }
    })
  } catch (error) {
    if (generation === reviewGeneration) {
      setStatusText('rewind-status', `Rewind failed: ${getErrorMessage(error)}`)
      appendLog('error', `fetch ${trackName}: ${getErrorMessage(error)}`)
    }
    return undefined
  }

  await waitForFetchIdle(() => lastArrival, generation)
  return generation === reviewGeneration ? frames : undefined
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

/// FETCH_OK only acknowledges the request. The objects follow on their own
/// stream and no completion event is surfaced, so wait for the arrivals to go
/// quiet before replaying them.
async function waitForFetchIdle(lastArrival: () => number, generation: number): Promise<void> {
  const deadline = performance.now() + FETCH_DEADLINE_MS
  while (generation === reviewGeneration && performance.now() < deadline) {
    if (performance.now() - lastArrival() > FETCH_IDLE_MS) {
      return
    }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
}

/// The window's audio is decoded up front. Frames are decoded a little ahead
/// of their presentation, not the whole window at once: decoded frames hold
/// GPU memory until they are shown. The function returns shortly before the
/// last frame is due so the next window is decoded in time to follow on.
async function playReview(frames: ReviewFrame[], audio: ReviewFrame[], generation: number): Promise<boolean> {
  const config = pendingReviewConfig()
  if (!config) {
    setStatusText('rewind-status', 'Rewind unavailable: the video track has no codec')
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

/// Fetched fragments are appended to a MediaSource on its own element while the
/// live one keeps playing hidden, so going back to live only swaps elements.
/// The next window is fetched once playback has caught up to within a few
/// seconds of what is buffered.
async function playReviewMse(frames: ReviewFrame[], audio: ReviewFrame[], generation: number): Promise<boolean> {
  if (!reviewMseOpened) {
    const source = subscribedCmafSource('video')
    if (!source) {
      setStatusText('rewind-status', 'Rewind unavailable: the CMAF track has no init segment')
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
    numberOfChannels: channelCount(track.channelConfig) ?? 2,
    description: track.initData ? base64ToUint8Array(track.initData) : undefined
  }
}

function backToLive(): void {
  reviewGeneration += 1
  reviewing = false
  seeking = false
  setPaused(false)
  reviewAnchorMicros = undefined
  reviewOriginMicros = undefined
  reviewPlayheadMicros = undefined
  reviewPlayout.stop()
  applyVolume()
  renderSeekbar()
  showPicture(livePicture())
  closeReviewMse()
  setStatusText('rewind-status', 'Live')
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

function playingMedia(): HTMLMediaElement[] {
  if (reviewing) {
    return reviewMse ? [reviewMse.element] : []
  }
  return mse ? [mse.element] : [element<HTMLVideoElement>('video')]
}

/// Review carries its own sound, so the live audio is silenced rather than
/// stopped while reviewing: it stays in step and is heard again the moment
/// playback returns to live.
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
  setStatusText('rewind-buffer', `${timeline.span.toFixed(1)}s`)
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

function renderReviewStatus(): void {
  const offset = reviewPlayout.syncOffsetMs()
  const sync = offset === undefined ? '' : ` · A/V ${formatSyncOffset(offset)}`
  setStatusText('rewind-status', `Rewound ${reviewBehindSeconds.toFixed(1)}s${sync}`)
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
