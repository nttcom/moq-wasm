import type { MOQTClient } from '../../pkg/moqt'
import { MediaTransportState } from './transportState'
import { buildLocHeader } from './loc'
import { monotonicUnixMicros } from './clock'

export interface AudioChunkSendOptions {
  chunk: EncodedAudioChunk
  captureTimestampMicros?: number
  trackAliases: bigint[]
  client: MOQTClient
  transportState: MediaTransportState
}

export async function sendAudioChunkViaMoqt({
  chunk,
  captureTimestampMicros,
  trackAliases,
  client,
  transportState
}: AudioChunkSendOptions): Promise<void> {
  if (!trackAliases.length) {
    return
  }

  const audio = transportState.audio
  for (const alias of trackAliases) {
    if (!audio.hasHeaderSent(alias, 0)) {
      await client.sendSubgroupHeader(alias, audio.groupId, 0n, 0)
      audio.markHeaderSent(alias, 0)
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
    await client.sendSubgroupObject(alias, audio.groupId, 0n, audio.nextObjectId, undefined, payload, locHeader)
  }

  audio.nextObjectId += 1n
}
