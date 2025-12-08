import type { MOQTClient } from '../../pkg/moqt_client_sample'
import { MediaTransportState } from './transportState'
import { serializeChunk, type ChunkMetadata } from './chunk'

export type VideoChunkSender = (
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedVideoChunk,
  client: MOQTClient,
  extraMetadata?: Partial<ChunkMetadata>
) => Promise<void>

export interface VideoChunkSendOptions {
  chunk: EncodedVideoChunk
  metadata: EncodedVideoChunkMetadata | undefined
  trackAliases: bigint[]
  publisherPriority: number
  client: MOQTClient
  transportState: MediaTransportState
  sender: VideoChunkSender
}

export async function sendVideoChunkViaMoqt({
  chunk,
  metadata,
  trackAliases,
  publisherPriority,
  client,
  transportState,
  sender
}: VideoChunkSendOptions): Promise<void> {
  if (!trackAliases.length) {
    return
  }

  const subgroupId = Number((metadata as { svc?: { temporalLayerId?: number } } | undefined)?.svc?.temporalLayerId ?? 0)
  transportState.ensureVideoSubgroup(subgroupId)

  const shouldIncludeCodec = trackAliases.some((alias) => transportState.shouldSendVideoCodec(alias))
  const decoderConfig = metadata?.decoderConfig as any
  const descriptionBase64 =
    shouldIncludeCodec && decoderConfig?.description ? bufferToBase64(decoderConfig.description) : undefined
  const extraMeta = shouldIncludeCodec
    ? {
        codec: decoderConfig?.codec ?? 'avc1.640028',
        descriptionBase64
      }
    : undefined

  if (chunk.type === 'key') {
    transportState.advanceVideoGroup()
    for (const alias of trackAliases) {
      for (const subgroup of transportState.listVideoSubgroups()) {
        await client.sendSubgroupStreamHeaderMessage(
          alias,
          transportState.getVideoGroupId(),
          BigInt(subgroup),
          publisherPriority
        )
        transportState.markVideoHeaderSent(alias, subgroup)
      }
    }
  } else {
    for (const alias of trackAliases) {
      if (!transportState.hasVideoHeaderSent(alias, subgroupId)) {
        await client.sendSubgroupStreamHeaderMessage(
          alias,
          transportState.getVideoGroupId(),
          BigInt(subgroupId),
          publisherPriority
        )
        transportState.markVideoHeaderSent(alias, subgroupId)
      }
    }
  }

  for (const alias of trackAliases) {
    await sender(
      alias,
      transportState.getVideoGroupId(),
      BigInt(subgroupId),
      transportState.getVideoObjectId(),
      chunk,
      client,
      extraMeta
    )
    if (shouldIncludeCodec) {
      transportState.markVideoCodecSent(alias)
    }
  }

  transportState.incrementVideoObject()
}

function bufferToBase64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf)
  let binary = ''
  for (let i = 0; i < bytes.byteLength; i++) {
    binary += String.fromCharCode(bytes[i])
  }
  return btoa(binary)
}
