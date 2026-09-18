import { bytesToBase64, type LocMetadata } from './loc'
import type { DeserializedChunk } from './chunk'

export function buildAudioChunkFromLoc(loc: LocMetadata, payload: Uint8Array): DeserializedChunk {
  return {
    metadata: { type: 'key', timestamp: loc.captureTimestampMicros ?? 0, duration: null },
    data: payload
  }
}

export function buildVideoChunkFromLoc(loc: LocMetadata, payload: Uint8Array, objectId: bigint): DeserializedChunk {
  return {
    metadata: {
      type: objectId === 0n ? 'key' : 'delta',
      timestamp: loc.captureTimestampMicros ?? 0,
      duration: null,
      descriptionBase64: loc.videoConfig ? bytesToBase64(loc.videoConfig) : undefined
    },
    data: payload
  }
}
