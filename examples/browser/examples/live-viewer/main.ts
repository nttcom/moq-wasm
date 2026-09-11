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
import { MseSink, type MseTrackSource, decodeBase64 } from '../../utils/media/mseSink'
import { getErrorMessage, initializeMediaExamplePage, parseTrackNamespace, setStatusText } from '../media/common'
import { MediaTimeline, formatElapsed } from './mediaTimeline'
import { GroupTimeline, type ReviewFrame, sortReviewFrames, toReviewFrame } from './rewind'

const AUTH_INFO = 'secret'
const ANNEX_B_FORMAT = 'annexb'
const TIMELINE_CAPACITY = 64
const REWIND_GROUP_COUNT = 4n
const FETCH_IDLE_MS = 400
const FETCH_DEADLINE_MS = 8_000
const CLOSED_GROUP_POLL_MS = 200
const REVIEW_PLAYHEAD_STEP_US = 1_000_000
const CMAF_TRACK_SUFFIX = '_cmaf'
const REVIEW_BUFFER_AHEAD_SECONDS = 8

type Packaging = 'loc' | 'cmaf'

type MediaKind = 'video' | 'audio'

type SubgroupObject = Parameters<Parameters<MoqtClientWrapper['setOnSubgroupObjectHandler']>[1]>[1]

type TrackSubscription = {
  requestId: bigint
  trackAlias: bigint
  name: string
}

type ReviewWindow = {
  start: bigint
  nextGroup: bigint
  frames: ReviewFrame[]
}

const moqtClient = new MoqtClientWrapper()
const videoDecoderWorker = new Worker(new URL('../../utils/media/decoders/videoDecoder.ts', import.meta.url), {
  type: 'module'
})
const audioDecoderWorker = new Worker(new URL('../../utils/media/decoders/audioDecoder.ts', import.meta.url), {
  type: 'module'
})

let videoTracks: MediaCatalogTrack[] = []
let audioTracks: MediaCatalogTrack[] = []
let cmafTracks: MediaCatalogTrack[] = []
let packaging: Packaging = 'loc'
let mse: MseSink | undefined
let liveStream: MediaStream | undefined
/// MSE decodes from the first random access point, so after a MediaSource is
/// (re)opened live fragments are dropped until one starts a group.
let cmafAwaitingKeyframe = true
let reviewMseOpened = false
const unstampedCmafGroups = new Set<bigint>()
const subscriptions = new Map<MediaKind, TrackSubscription>()
let videoWriter: WritableStreamDefaultWriter<VideoFrame> | undefined
let audioWriter: WritableStreamDefaultWriter<AudioData> | undefined
let videoObjectCount = 0
let receivedKbps = 0
const timeline = new GroupTimeline(TIMELINE_CAPACITY)
const mediaTimeline = new MediaTimeline()
let mediaTimelineTrackName: string | undefined
let reviewing = false
let reviewGeneration = 0
let reviewAnchorMicros: number | undefined
let reviewPlayheadMicros: number | undefined
let seeking = false
const seekbar = element<HTMLInputElement>('seekbar')

initializeMediaExamplePage('namespace')
element<HTMLButtonElement>('watchBtn').addEventListener('click', () => void watchStream())
element<HTMLButtonElement>('stopBtn').addEventListener('click', () => void stopStream())
element<HTMLSelectElement>('video-track').addEventListener('change', () => void resubscribe('video'))
element<HTMLSelectElement>('audio-track').addEventListener('change', () => void resubscribe('audio'))
element<HTMLInputElement>('bypass-jitter-buffer').addEventListener('change', applyDecoderConfig)
element<HTMLSelectElement>('packaging').addEventListener('change', () => void switchPackaging())
element<HTMLButtonElement>('rewind10Btn').addEventListener('click', () => void rewind(10))
element<HTMLButtonElement>('rewind30Btn').addEventListener('click', () => void rewind(30))
element<HTMLButtonElement>('liveBtn').addEventListener('click', backToLive)
element<HTMLButtonElement>('qualityBtn').addEventListener('click', () => toggleQualityMenu())
document.addEventListener('click', (event) => {
  const quality = element<HTMLDivElement>('quality-menu').parentElement
  if (quality && !quality.contains(event.target as Node)) {
    toggleQualityMenu(false)
  }
})
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') {
    toggleQualityMenu(false)
  }
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
/// longer holds.
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
  backToLive()
  closeMse()
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

async function subscribeCatalog(): Promise<void> {
  const namespace = trackNamespace()
  const { subscribeOk } = await moqtClient.subscribe(namespace, MEDIA_CATALOG_TRACK_NAME, AUTH_INFO, {
    forward: true
  })
  moqtClient.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (_groupId, object) => {
    const payload = new Uint8Array(object.objectPayload)
    if (payload.byteLength === 0) {
      return
    }
    void applyCatalog(new TextDecoder().decode(payload))
  })
  appendLog('info', `subscribed ${namespace.join('/')}/${MEDIA_CATALOG_TRACK_NAME}`)
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
  const { subscribeOk } = await moqtClient.subscribe(trackNamespace(), track.name, AUTH_INFO, {
    forward: true
  })
  moqtClient.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (_groupId, object) => {
    const payload = new Uint8Array(object.objectPayload)
    if (payload.byteLength === 0) {
      return
    }
    try {
      mediaTimeline.replace(new TextDecoder().decode(payload))
    } catch (error) {
      appendLog('error', `media timeline: ${getErrorMessage(error)}`)
      return
    }
    stampObservedCmafGroups()
    renderSeekbar()
  })
  appendLog('info', `subscribed ${trackNamespace().join('/')}/${track.name}`)
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
  const select = element<HTMLSelectElement>('packaging')
  const cmafOption = select.querySelector<HTMLOptionElement>('option[value="cmaf"]')
  if (cmafOption) {
    cmafOption.disabled = cmafTracks.length === 0
  }
}

async function switchPackaging(): Promise<void> {
  const selected = element<HTMLSelectElement>('packaging').value as Packaging
  if (selected === packaging) {
    return
  }
  backToLive()
  closeMse()
  await unsubscribeTrack('video')
  await unsubscribeTrack('audio')
  packaging = selected
  timeline.reset()
  unstampedCmafGroups.clear()
  if (packaging === 'loc') {
    const video = element<HTMLVideoElement>('video')
    video.removeAttribute('src')
    video.srcObject = liveStream ?? null
  }
  await resubscribe('video')
  await resubscribe('audio')
  await openLiveMse()
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
  return { mimeType: `${container}; codecs="${track.codec}"`, initSegment: decodeBase64(track.initData) }
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
  closeMse()
  const video = subscribedCmafSource('video')
  if (!video) {
    return
  }
  const audio = subscribedCmafSource('audio')
  mse = await MseSink.open(element<HTMLVideoElement>('video'), video, audio)
  cmafAwaitingKeyframe = true
}

function closeMse(): void {
  mse?.close()
  mse = undefined
  cmafAwaitingKeyframe = true
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
  }
  if (reviewing || !mse) {
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
    }
    if (reviewing && kind === 'video') {
      return
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

function applyDecoderConfig(): void {
  const config = {
    telemetryEnabled: true,
    bypassJitterBuffer: element<HTMLInputElement>('bypass-jitter-buffer').checked
  }
  videoDecoderWorker.postMessage({ type: 'config', config })
  audioDecoderWorker.postMessage({ type: 'config', config })
}

function startRendering(): void {
  applyDecoderConfig()
  const videoGenerator = new MediaStreamTrackGenerator({ kind: 'video' })
  const audioGenerator = new MediaStreamTrackGenerator({ kind: 'audio' })
  videoWriter = videoGenerator.writable.getWriter()
  audioWriter = audioGenerator.writable.getWriter()
  liveStream = new MediaStream([videoGenerator])
  element<HTMLVideoElement>('video').srcObject = liveStream
  element<HTMLAudioElement>('audio').srcObject = new MediaStream([audioGenerator])

  videoDecoderWorker.onmessage = async (event) => {
    if (event.data.type === 'bitrate') {
      receivedKbps = event.data.kbps ?? receivedKbps
      return
    }
    if (event.data.type !== 'frame') {
      return
    }
    const frame = event.data.frame as VideoFrame
    updateVideoStats(frame)
    if (!videoWriter || videoWriter.desiredSize === null || videoWriter.desiredSize <= 0) {
      frame.close()
      return
    }
    await videoWriter.ready
    await videoWriter.write(frame)
    frame.close()
  }

  audioDecoderWorker.onmessage = async (event) => {
    if (event.data.type !== 'audioData') {
      return
    }
    const audioData = event.data.audioData as AudioData
    if (!audioWriter) {
      audioData.close()
      return
    }
    await audioWriter.ready
    await audioWriter.write(audioData)
    audioData.close()
  }
}

function updateVideoStats(frame: VideoFrame): void {
  const stats = element<HTMLSpanElement>('video-stats')
  stats.textContent = `${frame.displayWidth}x${frame.displayHeight} · ${Math.round(receivedKbps)} kbps · ${videoObjectCount} objects`
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

async function rewind(seconds: number): Promise<void> {
  const target = timeline.resolveRewindTarget(seconds)
  if (!target) {
    setStatusText('rewind-status', 'Rewind unavailable: nothing buffered yet')
    return
  }

  const generation = ++reviewGeneration
  reviewing = true
  reviewMseOpened = false
  reviewAnchorMicros = target.captureMicros
  reviewPlayheadMicros = target.captureMicros
  renderSeekbar()
  setStatusText('playback-status', 'Reviewing')
  await review(target.groupId, generation)
}

/// Review playback is paced by capture timestamps, so it trails the live edge
/// until the viewer asks to go back. The FETCH for the next window is issued
/// while the current one plays, so a window boundary does not stall on the
/// request.
async function review(startGroup: bigint, generation: number): Promise<void> {
  let pending = await fetchReviewWindow(startGroup, generation)
  while (pending && generation === reviewGeneration) {
    const upcoming = fetchReviewWindow(pending.nextGroup, generation)
    setStatusText('rewind-status', `Rewound ${timeline.secondsBehindLive(pending.start).toFixed(1)}s`)
    const frames = sortReviewFrames(pending.frames)
    const played = packaging === 'cmaf' ? await playReviewMse(frames, generation) : await playReview(frames, generation)
    pending = played ? await upcoming : undefined
  }
}

async function fetchReviewWindow(start: bigint, generation: number): Promise<ReviewWindow | undefined> {
  const end = await awaitClosedWindowEnd(start, generation)
  const subscription = subscriptions.get('video')
  if (end === undefined || !subscription) {
    return undefined
  }

  const frames: ReviewFrame[] = []
  let lastArrival = performance.now()
  try {
    await moqtClient.fetch(trackNamespace(), subscription.name, start, 0n, end, 0n, {
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
      appendLog('error', `fetch: ${getErrorMessage(error)}`)
    }
    return undefined
  }

  await waitForFetchIdle(() => lastArrival, generation)
  if (generation !== reviewGeneration) {
    return undefined
  }
  if (frames.length === 0) {
    setStatusText('rewind-status', 'Rewind unavailable: no cached objects')
    return undefined
  }
  appendLog('info', `fetched ${frames.length} objects from group ${start}`)
  return { start, nextGroup: end + 1n, frames }
}

/// The live edge group is still open and a FETCH that reaches into it escapes
/// the relay cache, so a window can only end at the newest closed group. Once
/// playback has consumed those, wait for the publisher to close another one.
async function awaitClosedWindowEnd(start: bigint, generation: number): Promise<bigint | undefined> {
  while (generation === reviewGeneration) {
    const newestClosed = timeline.newestClosed
    if (newestClosed && start <= newestClosed.groupId) {
      const bounded = start + REWIND_GROUP_COUNT
      return bounded < newestClosed.groupId ? bounded : newestClosed.groupId
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

async function playReview(frames: ReviewFrame[], generation: number): Promise<boolean> {
  const canvas = element<HTMLCanvasElement>('review')
  const context = canvas.getContext('2d')
  const config = pendingReviewConfig()
  if (!context || !config) {
    setStatusText('rewind-status', 'Rewind unavailable: the video track has no codec')
    return false
  }

  element<HTMLVideoElement>('video').hidden = true
  canvas.hidden = false
  const origin = frames[0].captureMicros ?? 0
  const decoder = new VideoDecoder({
    output: (frame) => {
      if (generation !== reviewGeneration) {
        frame.close()
        return
      }
      canvas.width = frame.displayWidth
      canvas.height = frame.displayHeight
      context.drawImage(frame, 0, 0)
      advanceReviewPlayhead(origin + frame.timestamp)
      frame.close()
    },
    error: (error) => appendLog('error', `review decoder: ${error.message}`)
  })
  decoder.configure(config)

  for (const frame of frames) {
    if (generation !== reviewGeneration) {
      break
    }
    decoder.decode(
      new EncodedVideoChunk({
        type: frame.objectId === 0n ? 'key' : 'delta',
        timestamp: (frame.captureMicros ?? origin) - origin,
        data: frame.data
      })
    )
    await pace(frame, frames)
  }
  await decoder.flush().catch(() => undefined)
  decoder.close()
  return true
}

/// Fetched fragments are appended to a fresh MediaSource and the element plays
/// them itself; the next window is only fetched once playback has caught up to
/// within a few seconds of what is buffered.
async function playReviewMse(frames: ReviewFrame[], generation: number): Promise<boolean> {
  const video = element<HTMLVideoElement>('video')
  if (!reviewMseOpened) {
    const source = subscribedCmafSource('video')
    if (!source) {
      setStatusText('rewind-status', 'Rewind unavailable: the CMAF track has no init segment')
      return false
    }
    closeMse()
    mse = await MseSink.open(video, source, undefined)
    reviewMseOpened = true
    const anchor = reviewAnchorMicros ?? 0
    video.addEventListener('timeupdate', () => {
      if (generation === reviewGeneration) {
        advanceReviewPlayhead(anchor + video.currentTime * 1_000_000)
      }
    })
  }
  if (generation !== reviewGeneration || !mse) {
    return false
  }
  for (const frame of frames) {
    mse.appendVideo(frame.data)
  }
  while (generation === reviewGeneration) {
    const ahead = (mse.bufferedEnd() ?? 0) - video.currentTime
    if (ahead < REVIEW_BUFFER_AHEAD_SECONDS) {
      break
    }
    await new Promise((resolve) => setTimeout(resolve, CLOSED_GROUP_POLL_MS))
  }
  return generation === reviewGeneration
}

function pendingReviewConfig(): VideoDecoderConfig | undefined {
  const track = videoTracks.find((candidate) => candidate.name === subscriptions.get('video')?.name)
  if (!track?.codec) {
    return undefined
  }
  return { codec: track.codec, optimizeForLatency: true }
}

async function pace(frame: ReviewFrame, frames: ReviewFrame[]): Promise<void> {
  const next = frames[frames.indexOf(frame) + 1]
  const delayMs =
    next?.captureMicros !== undefined && frame.captureMicros !== undefined
      ? (next.captureMicros - frame.captureMicros) / 1_000
      : 0
  if (delayMs > 0) {
    await new Promise((resolve) => setTimeout(resolve, Math.min(delayMs, 1_000)))
  }
}

function backToLive(): void {
  reviewGeneration += 1
  reviewing = false
  seeking = false
  reviewAnchorMicros = undefined
  reviewPlayheadMicros = undefined
  renderSeekbar()
  element<HTMLCanvasElement>('review').hidden = true
  element<HTMLVideoElement>('video').hidden = false
  setStatusText('rewind-status', 'Live')
  if (reviewMseOpened) {
    reviewMseOpened = false
    void openLiveMse()
  }
}

/// The axis runs from the start of the broadcast, which the media timeline
/// places, so the bar keeps its meaning as cache retention grows. Until the
/// first timeline object arrives it falls back to the replayable window.
function toggleQualityMenu(open?: boolean): void {
  const menu = element<HTMLDivElement>('quality-menu')
  const expanded = open ?? menu.hidden
  menu.hidden = !expanded
  element<HTMLButtonElement>('qualityBtn').setAttribute('aria-expanded', String(expanded))
}

function renderSeekbar(): void {
  setStatusText('rewind-buffer', `${timeline.span.toFixed(1)}s`)
  element<HTMLButtonElement>('liveBtn').classList.toggle('reviewing', reviewing)
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
  seekbar.value = String(Math.min(latest, Math.max(Number(seekbar.min), anchor)))
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
  renderSeekbar()
}

function renderReviewProgress(anchor: number, playhead: number, latest: number): void {
  const played = element<HTMLDivElement>('seek-review-progress')
  const min = Number(seekbar.min)
  const axis = latest - min
  played.hidden = !reviewing || axis <= 0
  if (played.hidden) {
    return
  }
  played.style.left = `${(((anchor - min) / axis) * 100).toFixed(3)}%`
  played.style.width = `${((Math.max(0, playhead - anchor) / axis) * 100).toFixed(3)}%`
}

function renderReplayableWindow(replayableStart: number, latest: number): void {
  const min = Number(seekbar.min)
  const axis = latest - min
  const window = element<HTMLDivElement>('seek-available-window')
  const offset = axis > 0 ? (replayableStart - min) / axis : 0
  window.style.left = `${(offset * 100).toFixed(3)}%`
  window.style.width = `${((1 - offset) * 100).toFixed(3)}%`
  const elapsed = mediaTimeline.elapsedMsAt(min * 1_000_000)
  setStatusText('seek-start', elapsed === undefined ? '--:--' : formatElapsed(elapsed))
}

function liveEdgeSeconds(): number {
  return (timeline.latest?.captureMicros ?? 0) / 1_000_000
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
  if (timeline.latest) {
    void rewind(liveEdgeSeconds() - captureSeconds)
  }
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
