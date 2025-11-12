import type { MOQTClient } from '../../pkg/moqt_client_sample'
import { MediaTransportState } from './transportState'
import { serializeChunk } from './chunk'

export interface AudioChunkSendOptions {
  chunk: EncodedAudioChunk
  metadata: EncodedAudioChunkMetadata | undefined
  trackAliases: bigint[]
  client: MOQTClient
  transportState: MediaTransportState
}

export async function sendAudioChunkViaMoqt({
  chunk,
  metadata,
  trackAliases,
  client,
  transportState
}: AudioChunkSendOptions): Promise<void> {
  if (!trackAliases.length) {
    return
  }

  const subgroupId = 0
  transportState.ensureAudioSubgroup(subgroupId)

  for (const alias of trackAliases) {
    if (transportState.shouldSendAudioHeader(alias, subgroupId)) {
      await client.sendSubgroupStreamHeaderMessage(alias, transportState.getAudioGroupId(), BigInt(subgroupId), 0)
      transportState.markAudioHeaderSent(alias, subgroupId)
    }
  }

  const payload = serializeChunk({
    type: chunk.type,
    timestamp: chunk.timestamp,
    duration: chunk.duration ?? null,
    byteLength: chunk.byteLength,
    copyTo: (dest: Uint8Array) => chunk.copyTo(dest)
  })

  for (const alias of trackAliases) {
    await client.sendSubgroupStreamObject(
      alias,
      transportState.getAudioGroupId(),
      BigInt(subgroupId),
      transportState.getAudioObjectId(),
      undefined,
      payload
    )
  }

  transportState.incrementAudioObject()
}
