import { VideoJitterBuffer, type VideoJitterBufferEntry } from '../videoJitterBuffer'
import type { SubgroupWorkerMessage } from '../jitterBufferTypes'
import { createBitrateLogger } from '../bitrate'
import { readLocHeader } from '../loc'
import { buildVideoChunkFromLoc } from '../locChunk'

const bitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', kbps })
})

const POP_INTERVAL_MS = 33
const MISSING_META_WARN_SUPPRESS_AFTER_RESET_MS = 3000
const MAX_HOLD_MS = 30_000
/// A head that would wait this much longer than the hold means the source
/// clock ran away from the release timeline, so the timeline is rebuilt on it.
const HOLD_TIMELINE_SLACK_MS = 1_000

let videoDecoder: VideoDecoder | undefined
let waitingForKeyFrame = true
let newestKeyframeGroupId: bigint | undefined

/// Each group travels on its own stream, so the tail of a group can arrive
/// after the keyframe of the next group has been decoded. Its frames would
/// predict from references the keyframe has replaced and corrupt the picture
/// until the following keyframe, so they are dropped.
function isSupersededGroup(groupId: bigint): boolean {
  return newestKeyframeGroupId !== undefined && groupId < newestKeyframeGroupId
}

async function initializeVideoDecoder(config: VideoDecoderConfig) {
  const init: VideoDecoderInit = {
    output: postDecodedFrame,
    error: (e: any) => {
      console.warn('[videoDecoder] decoder error', e)
      videoDecoder = undefined
      cachedVideoConfig = null
      waitingForKeyFrame = true
      clearOutputQueue()
    }
  }
  try {
    const supported = await VideoDecoder.isConfigSupported(config)
    if (!supported.supported) {
      console.error('[videoDecoder] unsupported decoder config', config)
      return undefined
    }
  } catch (error) {
    console.error('[videoDecoder] failed to check decoder config support', error, config)
    return undefined
  }
  const decoder = new VideoDecoder(init)
  try {
    decoder.configure(config)
  } catch (error) {
    console.error('[videoDecoder] configure failed', error, config)
    try {
      decoder.close()
    } catch {
      // ignore close failure on configure error path
    }
    return undefined
  }
  return decoder
}

/// How long an object stays in the jitter buffer after arrival before it may
/// be decoded, so objects of an earlier group that arrive after a later
/// group's keyframe are still decoded in order. At zero objects are decoded
/// as they arrive.
let holdMs = 0
/// With a playout timeline from the page, an object is released this long
/// before it is due to be shown instead of `holdMs` after arrival.
let releaseMarginMs = 0

/// The page's playout clock, as capture time → wall time of presentation.
/// While it is known, objects are released `releaseMarginMs` before they are
/// due, which is what keeps them from arriving at the page already late.
let playoutTimeline: { captureMicros: number; dueAtUnixMs: number } | undefined

/// Without a playout timeline, objects leave the jitter buffer `holdMs` after
/// the first one arrived, each at its capture-time offset from that first one,
/// so an object that arrived late still leaves on schedule instead of late.
let holdTimeline: { captureMicros: number; releaseAtMs: number } | undefined

function holdReleaseAtMs(entry: VideoJitterBufferEntry): number {
  const byArrival = entry.receivedAtMs + holdMs
  if (entry.captureTimestampMicros === undefined) {
    return byArrival
  }
  if (playoutTimeline !== undefined) {
    const dueAtUnixMs =
      playoutTimeline.dueAtUnixMs + (entry.captureTimestampMicros - playoutTimeline.captureMicros) / 1000
    const releaseAtMs = performance.now() + (dueAtUnixMs - releaseMarginMs - Date.now())
    if (releaseAtMs <= byArrival + HOLD_TIMELINE_SLACK_MS) {
      return releaseAtMs
    }
  }
  if (
    holdTimeline === undefined ||
    holdTimeline.releaseAtMs + (entry.captureTimestampMicros - holdTimeline.captureMicros) / 1000 >
      byArrival + HOLD_TIMELINE_SLACK_MS
  ) {
    holdTimeline = { captureMicros: entry.captureTimestampMicros, releaseAtMs: byArrival }
  }
  return holdTimeline.releaseAtMs + (entry.captureTimestampMicros - holdTimeline.captureMicros) / 1000
}

const jitterBuffer = new VideoJitterBuffer()
let popTimer: ReturnType<typeof setTimeout> | undefined
type FrameIds = { groupId: bigint; objectId: bigint }
const outputPendingIds = new Map<number, FrameIds[]>()
let lastOutputMetaResetAtMs = Number.NEGATIVE_INFINITY

type CachedVideoConfig = { codec: string; descriptionBase64?: string }

let cachedVideoConfig: CachedVideoConfig | null = null
let catalogCodec: string | undefined
let catalogDescriptionBase64: string | undefined
let missingCatalogCodecWarned = false
let directDecodeQueue: Promise<void> = Promise.resolve()

function clearOutputQueue(preservePendingOutputIds = false): void {
  if (!preservePendingOutputIds) {
    outputPendingIds.clear()
  }
  lastOutputMetaResetAtMs = performance.now()
}

function schedulePop(delayMs: number): void {
  clearTimeout(popTimer)
  popTimer = setTimeout(pumpJitterBuffer, delayMs)
}

/// Switching to direct decoding must not let a fresh object overtake what
/// is still held, or the decoder sees P-frames out of order and smears.
function drainJitterBufferInOrder(): void {
  for (let entry = jitterBuffer.pop(); entry; entry = jitterBuffer.pop()) {
    decodeBufferedEntry(entry)
  }
}

function pumpJitterBuffer(): void {
  const head = jitterBuffer.peek()
  if (!head) {
    schedulePop(POP_INTERVAL_MS)
    return
  }
  const holdRemainingMs = holdReleaseAtMs(head) - performance.now()
  if (holdRemainingMs > 0) {
    schedulePop(holdRemainingMs)
    return
  }
  jitterBuffer.pop()
  decodeBufferedEntry(head)
}

function decodeBufferedEntry(entry: VideoJitterBufferEntry): void {
  const decodeQueueSize = videoDecoder?.decodeQueueSize ?? 0
  void submitForDecode(entry)
  schedulePop(decodeQueueSize > 12 ? POP_INTERVAL_MS : 0)
}

async function submitForDecode(entry: VideoJitterBufferEntry): Promise<void> {
  const { groupId, objectId, chunk } = entry
  if (waitingForKeyFrame && chunk.type !== 'key') {
    return
  }
  const pending = outputPendingIds.get(chunk.timestamp) ?? []
  pending.push({ groupId, objectId })
  outputPendingIds.set(chunk.timestamp, pending)
  await decode(entry)
}

function takeOutputIds(timestampUs: number): FrameIds | undefined {
  const pending = outputPendingIds.get(timestampUs)
  const ids = pending?.shift()
  if (pending?.length === 0) {
    outputPendingIds.delete(timestampUs)
  }
  return ids
}

function postDecodedFrame(frame: VideoFrame): void {
  const ids = takeOutputIds(frame.timestamp)
  if (!ids) {
    if (performance.now() - lastOutputMetaResetAtMs > MISSING_META_WARN_SUPPRESS_AFTER_RESET_MS) {
      console.warn('[videoDecoder][output] missing meta for decoded frame', { timestampUs: frame.timestamp })
    }
    frame.close()
    return
  }
  self.postMessage({ type: 'frame', frame, ...ids }, [frame])
}

schedulePop(POP_INTERVAL_MS)

type WorkerMessage =
  | SubgroupWorkerMessage
  | { type: 'config'; holdMs: number; releaseMarginMs: number }
  | { type: 'catalog'; codec?: string; descriptionBase64?: string }
  | { type: 'timeline'; captureMicros?: number; dueAtUnixMs?: number }

self.onmessage = (event: MessageEvent<WorkerMessage>) => {
  const data = event.data
  switch (data.type) {
    case 'config':
      if (data.holdMs === 0 && holdMs > 0) {
        drainJitterBufferInOrder()
      }
      holdMs = Math.min(data.holdMs, MAX_HOLD_MS)
      releaseMarginMs = data.releaseMarginMs
      holdTimeline = undefined
      playoutTimeline = undefined
      newestKeyframeGroupId = undefined
      clearOutputQueue()
      return
    case 'catalog':
      applyCatalogInfo(data.codec, data.descriptionBase64)
      return
    case 'timeline':
      playoutTimeline =
        data.captureMicros !== undefined && data.dueAtUnixMs !== undefined
          ? { captureMicros: data.captureMicros, dueAtUnixMs: data.dueAtUnixMs }
          : undefined
      schedulePop(0)
      return
    case 'object': {
      const { groupId, subgroupStreamObject: object } = data
      bitrateLogger.addBytes(object.objectPayloadLength)
      if (!object.objectPayloadLength) {
        return
      }
      const loc = readLocHeader(object.locHeader)
      const entry: VideoJitterBufferEntry = {
        groupId,
        subgroupId: object.subgroupId ?? 0n,
        objectId: object.objectId,
        captureTimestampMicros: loc.captureTimestampMicros,
        receivedAtMs: performance.now(),
        chunk: buildVideoChunkFromLoc(loc, object.objectPayload, object.objectId)
      }
      if (holdMs === 0) {
        enqueueDirectDecode(entry)
        return
      }
      jitterBuffer.push(entry)
      schedulePop(0)
    }
  }
}

function enqueueDirectDecode(entry: VideoJitterBufferEntry): void {
  directDecodeQueue = directDecodeQueue
    .then(() => submitForDecode(entry))
    .catch((error) => {
      console.error('[videoDecoder] direct decode failed', error)
    })
}

async function decode({ groupId, objectId, chunk }: VideoJitterBufferEntry) {
  if (isSupersededGroup(groupId)) {
    return
  }

  const resolvedConfig = resolveVideoConfig(chunk.descriptionBase64)
  if (!resolvedConfig) {
    return
  }

  const encodedVideoChunk = new EncodedVideoChunk({ type: chunk.type, timestamp: chunk.timestamp, data: chunk.data })

  if (!videoDecoder || videoDecoder.state === 'closed') {
    videoDecoder = await initializeVideoDecoder(buildVideoDecoderConfig(resolvedConfig))
    if (!videoDecoder) {
      return
    }
    cachedVideoConfig = resolvedConfig
    waitingForKeyFrame = true
    // The current keyframe's output IDs were registered before decode();
    // preserve them across first decoder configuration so the first frame is not dropped.
    clearOutputQueue(true)
    if (chunk.type !== 'key') {
      return
    }
  }

  if (waitingForKeyFrame && chunk.type !== 'key') {
    return
  }

  try {
    videoDecoder.decode(encodedVideoChunk)
    if (chunk.type === 'key') {
      waitingForKeyFrame = false
      newestKeyframeGroupId = groupId
    }
  } catch (error) {
    if (isKeyFrameRequiredError(error)) {
      waitingForKeyFrame = true
      clearOutputQueue()
      console.warn('[videoDecoder] decode requires key frame; waiting for next key frame', {
        groupId: groupId.toString(),
        objectId: objectId.toString(),
        chunkType: chunk.type
      })
      return
    }
    throw error
  }
}

function isKeyFrameRequiredError(error: unknown): boolean {
  if (!(error instanceof DOMException)) {
    return false
  }
  return error.name === 'DataError' && error.message.includes('A key frame is required')
}

function applyCatalogInfo(codec?: string, descriptionBase64?: string): void {
  newestKeyframeGroupId = undefined
  if (descriptionBase64) {
    catalogDescriptionBase64 = descriptionBase64
  }
  if (!codec || catalogCodec === codec) {
    return
  }
  catalogCodec = codec
  if (cachedVideoConfig && cachedVideoConfig.codec !== codec) {
    console.warn('[videoDecoder] catalog codec changed after decoder initialization; ignoring new codec', {
      initializedCodec: cachedVideoConfig.codec,
      catalogCodec: codec
    })
  }
}

function resolveVideoConfig(descriptionBase64: string | undefined): CachedVideoConfig | null {
  if (cachedVideoConfig) {
    return cachedVideoConfig
  }
  if (!catalogCodec) {
    if (!missingCatalogCodecWarned) {
      missingCatalogCodecWarned = true
      console.warn('[videoDecoder] skip decode until catalog codec is available')
    }
    return null
  }
  missingCatalogCodecWarned = false
  return { codec: catalogCodec, descriptionBase64: descriptionBase64 ?? catalogDescriptionBase64 }
}

function buildVideoDecoderConfig(resolved: CachedVideoConfig): VideoDecoderConfig {
  return {
    codec: resolved.codec,
    description: resolved.descriptionBase64 ? Uint8Array.fromBase64(resolved.descriptionBase64) : undefined,
    hardwareAcceleration: 'prefer-software'
  }
}
