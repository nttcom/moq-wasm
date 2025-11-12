import type { MOQTClient } from '../../pkg/moqt_client_sample'
import { MediaTransportState } from './transportState'

export type VideoChunkSender = (
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedVideoChunk,
  client: MOQTClient
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
      client
    )
  }

  transportState.incrementVideoObject()
}
