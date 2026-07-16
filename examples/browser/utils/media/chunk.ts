export type ChunkMetadata = {
  type: string
  timestamp: number
  duration: number | null
  codec?: string
  descriptionBase64?: string
  avcFormat?: 'annexb' | 'avc'
  sampleRate?: number
  channels?: number
}

export type DeserializedChunk = {
  metadata: ChunkMetadata
  data: Uint8Array
}
