import type { SubgroupSender } from '@moqt/moqtClient'
import { MediaTransportState } from './transportState'
import { type AudioLevel, buildLocHeader } from './loc'
import { monotonicUnixMicros } from './clock'
import { OBJECT_STATUS_END_OF_GROUP } from './objectStatus'

export interface AudioChunkSendOptions {
  chunk: EncodedAudioChunk
  captureTimestampMicros?: number
  audioLevel?: AudioLevel
  trackAliases: bigint[]
  client: SubgroupSender
  transportState: MediaTransportState
}

export async function sendAudioChunkViaMoqt({
  chunk,
  captureTimestampMicros,
  audioLevel,
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
      : (audio.nextCaptureTimestampMicros ?? monotonicUnixMicros())
  audio.nextCaptureTimestampMicros = resolvedCaptureTimestampMicros + (chunk.duration ?? 0)
  const locHeader = buildLocHeader({
    captureTimestampMicros: resolvedCaptureTimestampMicros,
    audioLevel
  })

  for (const alias of trackAliases) {
    await client.sendSubgroupObject(alias, audio.groupId, 0n, audio.nextObjectId, undefined, payload, locHeader)
  }

  audio.nextObjectId += 1n
}

/// Skips an alias with no subgroup stream open in the current group, such as
/// one that subscribed after the group's last object.
export async function sendAudioEndOfGroupViaMoqt(
  trackAliases: bigint[],
  client: SubgroupSender,
  transportState: MediaTransportState
): Promise<void> {
  const audio = transportState.audio
  for (const alias of trackAliases) {
    if (!audio.hasHeaderSent(alias, 0)) {
      continue
    }
    await client.sendSubgroupObject(
      alias,
      audio.groupId,
      0n,
      audio.nextObjectId,
      OBJECT_STATUS_END_OF_GROUP,
      new Uint8Array(0),
      undefined
    )
  }
}
