import { bytesToBase64, readLocHeader } from './loc'
import type { DeserializedChunk } from './chunk'
import type { SubgroupObjectWithLoc } from './jitterBufferTypes'

export function getCaptureTimestampMicros(value: number | undefined): number | undefined {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    return undefined
  }
  return value
}

export function buildAudioChunkFromLoc(object: SubgroupObjectWithLoc): DeserializedChunk {
  const loc = readLocHeader(object.locHeader)
  const captureMicros = getCaptureTimestampMicros(loc.captureTimestampMicros)
  return {
    metadata: {
      type: 'key',
      timestamp: typeof captureMicros === 'number' ? captureMicros : 0,
      duration: null
    },
    data: object.objectPayload
  }
}

export function buildVideoChunkFromLoc(object: SubgroupObjectWithLoc, objectId: bigint): DeserializedChunk {
  const loc = readLocHeader(object.locHeader)
  const captureMicros = getCaptureTimestampMicros(loc.captureTimestampMicros)
  return {
    metadata: {
      type: objectId === 0n ? 'key' : 'delta',
      timestamp: typeof captureMicros === 'number' ? captureMicros : 0,
      duration: null,
      descriptionBase64: loc.videoConfig ? bytesToBase64(loc.videoConfig) : undefined
    },
    data: object.objectPayload
  }
}
