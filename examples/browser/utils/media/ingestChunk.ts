import { type LocHeader, readLocHeader } from './loc'

/// The live-ingest bridge prefixes each object with its own metadata:
/// `[meta_len (u32 BE)][meta JSON][coded data]`. Browser publishers instead send
/// the coded data alone and carry metadata in MoQT LOC extension headers.
type IngestChunkMetadata = {
  type?: string
  timestamp?: number
  duration?: number
  sentAt?: number
  codec?: string | null
  descriptionBase64?: string | null
  sampleRate?: number
  channels?: number
}

export type IngestChunk = {
  data: Uint8Array
  locHeader?: LocHeader
  codec?: string
  descriptionBase64?: string
  sampleRate?: number
  channels?: number
}

const META_LENGTH_BYTES = 4
const JSON_OBJECT_START = 0x7b

export function parseIngestChunk(payload: Uint8Array, locHeader?: LocHeader): IngestChunk {
  const metadata = readMetadata(payload)
  if (!metadata) {
    return { data: payload, locHeader }
  }

  const [meta, dataOffset] = metadata
  return {
    data: payload.subarray(dataOffset),
    locHeader: resolveLocHeader(meta, locHeader),
    codec: meta.codec ?? undefined,
    descriptionBase64: meta.descriptionBase64 ?? undefined,
    sampleRate: meta.sampleRate,
    channels: meta.channels
  }
}

function readMetadata(payload: Uint8Array): [IngestChunkMetadata, number] | undefined {
  if (payload.byteLength <= META_LENGTH_BYTES || payload[META_LENGTH_BYTES] !== JSON_OBJECT_START) {
    return undefined
  }

  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength)
  const metaLength = view.getUint32(0)
  const dataOffset = META_LENGTH_BYTES + metaLength
  if (metaLength === 0 || dataOffset > payload.byteLength) {
    return undefined
  }

  try {
    const meta = JSON.parse(new TextDecoder().decode(payload.subarray(META_LENGTH_BYTES, dataOffset)))
    return [meta as IngestChunkMetadata, dataOffset]
  } catch (_error) {
    return undefined
  }
}

/// The bridge sends empty extension headers, so the timestamp it packed into
/// the metadata envelope is the only capture time available.
function resolveLocHeader(meta: IngestChunkMetadata, locHeader?: LocHeader): LocHeader | undefined {
  if (readLocHeader(locHeader).captureTimestampMicros !== undefined) {
    return locHeader
  }
  return buildLocHeaderFromMetadata(meta) ?? locHeader
}

function buildLocHeaderFromMetadata(meta: IngestChunkMetadata): LocHeader | undefined {
  if (typeof meta.sentAt !== 'number') {
    return undefined
  }

  return {
    extensions: [{ type: 'captureTimestamp', value: { microsSinceUnixEpoch: Math.round(meta.sentAt * 1000) } }]
  } as LocHeader
}
