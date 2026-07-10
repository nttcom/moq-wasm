import type { SubgroupObjectMessage } from '../../../../pkg/moqt_client_wasm'
import type { CameraId } from '../types/monitoring'
import { tryDeserializeChunk } from '../../../../utils/media/chunk'

const log = (...args: unknown[]) => console.log('[mon][decoder]', ...args)

function base64ToUint8Array(base64: string): Uint8Array {
  const binary = atob(base64)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
  return bytes
}

export class CameraSubscriber {
  private readonly worker: Worker
  private canvas: HTMLCanvasElement | null = null
  private frameCount = 0
  readonly camId: CameraId

  firstReceivedGroupId: bigint | null = null
  latestReceivedGroupId: bigint | null = null
  onGroupIdUpdate: ((first: bigint, latest: bigint) => void) | null = null
  paused = false

  private liveResuming = false
  private liveResumingSkipUntil: bigint | null = null
  lastSentGroupId: bigint | null = null
  lastSentObjectId: bigint = 0n

  private gopBuffer: Array<{
    subgroupId: bigint | undefined
    objectIdDelta: bigint
    objectPayloadLength: number
    objectStatus: number | undefined
    locHeader: unknown
    payload: Uint8Array
  }> = []
  private gopBufferGroupId: bigint | null = null

  private reviewDecoder: VideoDecoder | null = null
  private reviewGeneration = 0

  constructor(camId: CameraId) {
    this.camId = camId
    this.worker = new Worker(new URL('../../../../utils/media/decoders/videoDecoder.ts', import.meta.url), {
      type: 'module'
    })
    // enable jitter buffer + capture-timestamp pacing (was bypassed for lowest latency)
    this.worker.postMessage({ type: 'config', config: { bypassJitterBuffer: false, telemetryEnabled: false } })
    this.worker.postMessage({ type: 'catalog', codec: 'avc3.640028', framerate: 30 })

    this.worker.onmessage = (e) => {
      if (e.data.type === 'frame') {
        this.frameCount++
        if (this.frameCount === 1) log('first frame decoded', { camId })
        this.drawFrame(e.data.frame)
      }
    }
    this.worker.onerror = (e) => console.error('[mon][decoder] worker error', e)
    log('created', { camId })
  }

  setCanvas(canvas: HTMLCanvasElement | null): void {
    this.canvas = canvas
  }

  handleObject(groupId: bigint, msg: SubgroupObjectMessage): void {
    if (this.firstReceivedGroupId === null) {
      log('first object received', { camId: this.camId, groupId: groupId.toString() })
      this.firstReceivedGroupId = groupId
    }
    if (this.latestReceivedGroupId === null || groupId > this.latestReceivedGroupId) {
      this.latestReceivedGroupId = groupId
      this.onGroupIdUpdate?.(this.firstReceivedGroupId!, this.latestReceivedGroupId)
    }

    // GoP バッファ更新（review 中も継続して最新 GoP を保持）
    if (this.gopBufferGroupId === null || groupId > this.gopBufferGroupId) {
      this.gopBuffer = []
      this.gopBufferGroupId = groupId
    }
    if (groupId === this.gopBufferGroupId && msg.objectPayloadLength > 0) {
      this.gopBuffer.push({
        subgroupId: msg.subgroupId,
        objectIdDelta: msg.objectIdDelta,
        objectPayloadLength: msg.objectPayloadLength,
        objectStatus: msg.objectStatus as number,
        locHeader: msg.locHeader,
        payload: new Uint8Array(msg.objectPayload)
      })
    }

    if (this.paused) return

    if (this.liveResuming) {
      if (this.lastSentGroupId !== null && groupId <= this.lastSentGroupId) return
      if (this.liveResumingSkipUntil === null) {
        // gopBuffer にこの groupId のオブジェクトがある = review 中に受信済み = mid-GoP の可能性
        // ない = 今まさに始まった新しい GoP = objectId=0 (I フレーム) が保証される
        if (this.gopBufferGroupId !== null && groupId <= this.gopBufferGroupId) {
          this.liveResumingSkipUntil = groupId
          return
        }
        // 新しい GoP の先頭 → スキップ不要
        this.liveResuming = false
      } else {
        if (groupId <= this.liveResumingSkipUntil) return
        this.liveResuming = false
        this.liveResumingSkipUntil = null
      }
    }

    // Track objectId: reset on new group, increment within group (skip empty end-of-group markers)
    if (groupId !== this.lastSentGroupId) {
      this.lastSentObjectId = 0n
    } else if (msg.objectPayloadLength > 0) {
      this.lastSentObjectId += 1n
    }
    this.lastSentGroupId = groupId

    const payload = new Uint8Array(msg.objectPayload)
    this.worker.postMessage(
      {
        groupId,
        subgroupStreamObject: {
          subgroupId: msg.subgroupId,
          objectIdDelta: msg.objectIdDelta,
          objectPayloadLength: msg.objectPayloadLength,
          objectPayload: payload,
          objectStatus: msg.objectStatus,
          locHeader: msg.locHeader
        }
      },
      [payload.buffer]
    )
  }

  private drawFrame(frame: VideoFrame): void {
    const canvas = this.canvas
    if (!canvas) {
      frame.close()
      return
    }
    if (canvas.width !== frame.displayWidth) canvas.width = frame.displayWidth
    if (canvas.height !== frame.displayHeight) canvas.height = frame.displayHeight
    const ctx = canvas.getContext('2d')
    if (ctx) ctx.drawImage(frame, 0, 0)
    frame.close()
  }

  resumeLive(): void {
    if (this.reviewDecoder && this.reviewDecoder.state !== 'closed') {
      this.reviewDecoder.close()
      this.reviewDecoder = null
    }
    if (this.canvas) {
      this.canvas.getContext('2d')?.clearRect(0, 0, this.canvas.width, this.canvas.height)
    }
    if (this.gopBufferGroupId !== null && this.gopBuffer.length > 0) {
      log('replaying GoP buffer', {
        camId: this.camId,
        groupId: this.gopBufferGroupId.toString(),
        frames: this.gopBuffer.length
      })
      let replayedObjectId = 0n
      for (let i = 0; i < this.gopBuffer.length; i++) {
        const obj = this.gopBuffer[i]
        // MoQT: 先頭オブジェクトの objectId = objectIdDelta、以降は前の objectId + objectIdDelta
        replayedObjectId = i === 0 ? obj.objectIdDelta : replayedObjectId + obj.objectIdDelta
        const payload = new Uint8Array(obj.payload)
        this.worker.postMessage(
          {
            groupId: this.gopBufferGroupId,
            subgroupStreamObject: {
              subgroupId: obj.subgroupId,
              objectIdDelta: obj.objectIdDelta,
              objectPayloadLength: obj.objectPayloadLength,
              objectPayload: payload,
              objectStatus: obj.objectStatus,
              locHeader: obj.locHeader
            }
          },
          [payload.buffer]
        )
      }
      this.lastSentGroupId = this.gopBufferGroupId
      this.lastSentObjectId = replayedObjectId
      this.liveResuming = false
      this.liveResumingSkipUntil = null
    } else {
      this.liveResuming = true
      this.liveResumingSkipUntil = null
    }

    this.paused = false
  }

  decodeFrame(groupId: bigint, payload: Uint8Array): void {
    const parsed = tryDeserializeChunk(payload)
    if (!parsed) {
      log('decodeFrame: failed to parse payload', { camId: this.camId, groupId })
      return
    }

    if (this.reviewDecoder && this.reviewDecoder.state !== 'closed') {
      this.reviewDecoder.close()
    }

    const gen = ++this.reviewGeneration
    const { metadata, data } = parsed

    const config: VideoDecoderConfig = {
      codec: metadata.codec ?? 'avc1.640028',
      ...(metadata.descriptionBase64 ? { description: base64ToUint8Array(metadata.descriptionBase64) } : {}),
      ...(metadata.codec?.startsWith('avc')
        ? { avc: { format: (metadata.avcFormat ?? 'avc') as AvcBitstreamFormat } }
        : {})
    }

    this.reviewDecoder = new VideoDecoder({
      output: (frame) => {
        if (this.reviewGeneration !== gen) {
          frame.close()
          return
        }
        const canvas = this.canvas
        if (canvas) {
          if (canvas.width !== frame.displayWidth) canvas.width = frame.displayWidth
          if (canvas.height !== frame.displayHeight) canvas.height = frame.displayHeight
          canvas.getContext('2d')?.drawImage(frame, 0, 0)
        }
        frame.close()
      },
      error: (e) => console.error('[mon][review] decoder error', { camId: this.camId, groupId, e })
    })

    this.reviewDecoder.configure(config)
    this.reviewDecoder.decode(
      new EncodedVideoChunk({
        type: metadata.type as EncodedVideoChunkType,
        timestamp: metadata.timestamp,
        duration: metadata.duration ?? undefined,
        data
      })
    )
  }

  // Decode I-frame through target P-frame sequentially, displaying only the target frame.
  decodeFrameSequential(frames: Map<bigint, Uint8Array>, targetObjectId: bigint): void {
    const iFramePayload = frames.get(0n)
    if (!iFramePayload) return

    // If target is the I-frame itself, fall through to decodeFrame
    if (targetObjectId === 0n) {
      this.decodeFrame(0n, iFramePayload)
      return
    }

    const iFrameParsed = tryDeserializeChunk(iFramePayload)
    if (!iFrameParsed) return

    const targetPayload = frames.get(targetObjectId)
    const targetParsed = targetPayload ? tryDeserializeChunk(targetPayload) : null
    // Use timestamp to identify the target frame in the output callback
    const targetTimestamp = targetParsed?.metadata.timestamp ?? null

    if (this.reviewDecoder && this.reviewDecoder.state !== 'closed') {
      this.reviewDecoder.close()
    }

    const gen = ++this.reviewGeneration

    const config: VideoDecoderConfig = {
      codec: iFrameParsed.metadata.codec ?? 'avc1.640028',
      ...(iFrameParsed.metadata.descriptionBase64
        ? { description: base64ToUint8Array(iFrameParsed.metadata.descriptionBase64) }
        : {}),
      ...(iFrameParsed.metadata.codec?.startsWith('avc')
        ? { avc: { format: (iFrameParsed.metadata.avcFormat ?? 'avc') as AvcBitstreamFormat } }
        : {})
    }

    this.reviewDecoder = new VideoDecoder({
      output: (frame) => {
        if (this.reviewGeneration !== gen) {
          frame.close()
          return
        }
        if (targetTimestamp !== null && frame.timestamp !== targetTimestamp) {
          frame.close()
          return
        }
        const canvas = this.canvas
        if (canvas) {
          if (canvas.width !== frame.displayWidth) canvas.width = frame.displayWidth
          if (canvas.height !== frame.displayHeight) canvas.height = frame.displayHeight
          canvas.getContext('2d')?.drawImage(frame, 0, 0)
        }
        frame.close()
      },
      error: (e) => console.error('[mon][review] sequential decoder error', { camId: this.camId, targetObjectId, e })
    })

    this.reviewDecoder.configure(config)

    for (let objId = 0n; objId <= targetObjectId; objId++) {
      const payload = frames.get(objId)
      if (!payload) {
        log('decodeFrameSequential: missing frame, stopping', { camId: this.camId, objId, targetObjectId })
        break
      }
      const parsed = tryDeserializeChunk(payload)
      if (!parsed) break
      try {
        this.reviewDecoder.decode(
          new EncodedVideoChunk({
            type: parsed.metadata.type as EncodedVideoChunkType,
            timestamp: parsed.metadata.timestamp,
            duration: parsed.metadata.duration ?? undefined,
            data: parsed.data
          })
        )
      } catch (e) {
        console.error('[mon][review] sequential decode error', { camId: this.camId, objId, e })
        break
      }
    }
  }

  dispose(): void {
    log('disposing', { camId: this.camId })
    this.worker.terminate()
    if (this.reviewDecoder && this.reviewDecoder.state !== 'closed') {
      this.reviewDecoder.close()
    }
    this.reviewDecoder = null
  }
}
