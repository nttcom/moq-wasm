import type { MOQTClient } from '../../pkg/moqt_client_wasm'
import { MediaTransportState } from './transportState'
import { buildLocHeader, arrayBufferToUint8Array, type LocHeader } from './loc'
import { monotonicUnixMicros } from './clock'

export type VideoChunkSender = (
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedVideoChunk,
  client: MOQTClient,
  locHeader?: LocHeader
) => Promise<void>

export interface VideoChunkSendOptions {
  chunk: EncodedVideoChunk
  metadata: EncodedVideoChunkMetadata | undefined
  captureTimestampMicros?: number
  trackAliases: bigint[]
  publisherPriority: number
  client: MOQTClient
  transportState: MediaTransportState
  sender: VideoChunkSender
}

export async function sendVideoChunkViaMoqt({
  chunk,
  metadata,
  captureTimestampMicros,
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
  const configBytes = arrayBufferToUint8Array(decoderConfig?.description)
  const includeConfig = chunk.type === 'key' || shouldIncludeCodec
  const resolvedCaptureTimestampMicros =
    typeof captureTimestampMicros === 'number' && Number.isFinite(captureTimestampMicros)
      ? Math.round(captureTimestampMicros)
      : monotonicUnixMicros()
  const locHeader = buildLocHeader({
    captureTimestampMicros: resolvedCaptureTimestampMicros,
    videoConfig: includeConfig ? configBytes : undefined
  })

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
      locHeader
    )
    if (includeConfig && configBytes) {
      transportState.markVideoCodecSent(alias)
    }
  }

  transportState.incrementVideoObject()
}
