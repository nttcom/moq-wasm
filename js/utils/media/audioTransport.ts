import type { MOQTClient } from '../../pkg/moqt_client_wasm'
import { MediaTransportState } from './transportState'
import { buildLocHeader } from './loc'

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

  const payload = new Uint8Array(chunk.byteLength)
  chunk.copyTo(payload)
  const locHeader = buildLocHeader({
    captureTimestampMicros: Date.now() * 1000
  })

  for (const alias of trackAliases) {
    await client.sendSubgroupStreamObject(
      alias,
      transportState.getAudioGroupId(),
      BigInt(subgroupId),
      transportState.getAudioObjectId(),
      undefined,
      payload,
      locHeader
    )
  }

  transportState.incrementAudioObject()
}
