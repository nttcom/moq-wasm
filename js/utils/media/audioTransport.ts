import type { MOQTClient } from '../../pkg/moqt_client_wasm'
import { MediaTransportState } from './transportState'
import { buildLocHeader } from './loc'
import { monotonicUnixMicros } from './clock'

export interface AudioChunkSendOptions {
  chunk: EncodedAudioChunk
  metadata: EncodedAudioChunkMetadata | undefined
  captureTimestampMicros?: number
  trackAliases: bigint[]
  client: MOQTClient
  transportState: MediaTransportState
}

export async function sendAudioChunkViaMoqt({
  chunk,
  metadata,
  captureTimestampMicros,
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
  const resolvedCaptureTimestampMicros =
    typeof captureTimestampMicros === 'number' && Number.isFinite(captureTimestampMicros)
      ? Math.round(captureTimestampMicros)
      : monotonicUnixMicros()
  const locHeader = buildLocHeader({
    captureTimestampMicros: resolvedCaptureTimestampMicros
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
