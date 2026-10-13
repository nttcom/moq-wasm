import type { MOQTClient } from '../../pkg/moqt'
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

  if (transportState.audio.groupId < 0n) {
    transportState.audio.advanceGroup()
  }

  const subgroupId = 0
  transportState.audio.ensureSubgroup(subgroupId)
  if (transportState.audio.groupId < 0n) {
    transportState.audio.advanceGroup()
  }

  for (const alias of trackAliases) {
    if (!transportState.audio.hasHeaderSent(alias, subgroupId)) {
      await client.sendSubgroupHeader(alias, transportState.audio.groupId, BigInt(subgroupId), 0)
      transportState.audio.markHeaderSent(alias, subgroupId)
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
    await client.sendSubgroupObject(
      alias,
      transportState.audio.groupId,
      BigInt(subgroupId),
      transportState.audio.nextObjectId,
      undefined,
      payload,
      locHeader
    )
  }

  transportState.audio.nextObjectId += 1n
}
