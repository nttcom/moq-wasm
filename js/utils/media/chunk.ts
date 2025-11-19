const META_LENGTH_BYTES = 4

export type ChunkMetadata = {
  type: string
  timestamp: number
  duration: number | null
  sentAt?: number
}

export type DeserializedChunk = {
  metadata: ChunkMetadata
  data: Uint8Array
}

export type EncodedChunkLike = {
  type: string
  timestamp: number
  duration?: number | null
  byteLength: number
  copyTo: (dest: Uint8Array) => void
}

export function serializeChunk(chunk: EncodedChunkLike): Uint8Array {
  const metadata: ChunkMetadata = {
    type: chunk.type,
    timestamp: chunk.timestamp,
    duration: chunk.duration ?? null,
    sentAt: Date.now()
  }
  const metaBytes = new TextEncoder().encode(JSON.stringify(metadata))
  const payload = new Uint8Array(META_LENGTH_BYTES + metaBytes.length + chunk.byteLength)
  const view = new DataView(payload.buffer)
  view.setUint32(0, metaBytes.length)
  payload.set(metaBytes, META_LENGTH_BYTES)

  const data = new Uint8Array(chunk.byteLength)
  chunk.copyTo(data)
  payload.set(data, META_LENGTH_BYTES + metaBytes.length)
  return payload
}

export function deserializeChunk(payload: Uint8Array): DeserializedChunk {
  if (payload.byteLength < META_LENGTH_BYTES) {
    throw new Error('Payload too small to contain metadata')
  }
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength)
  const metaLength = view.getUint32(0)
  const totalMetaLength = META_LENGTH_BYTES + metaLength
  if (payload.byteLength < totalMetaLength) {
    throw new Error('Payload too small to contain metadata')
  }
  const metaBytes = payload.slice(META_LENGTH_BYTES, META_LENGTH_BYTES + metaLength)
  const metadata = JSON.parse(new TextDecoder().decode(metaBytes)) as ChunkMetadata
  const data = payload.slice(META_LENGTH_BYTES + metaLength)
  return { metadata, data }
}
