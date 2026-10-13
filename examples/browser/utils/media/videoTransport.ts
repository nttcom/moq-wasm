import type { MOQTClient } from '../../pkg/moqt'
import { MediaTransportState } from './transportState'
import { buildLocHeader, type LocHeader } from './loc'
import { monotonicUnixMicros } from './clock'
import { OBJECT_STATUS_END_OF_GROUP } from './objectStatus'

export type VideoChunkSender = (
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectNumber: bigint,
  payload: Uint8Array,
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
  sender?: VideoChunkSender
}

export async function sendVideoChunkViaMoqt({
  chunk,
  metadata,
  captureTimestampMicros,
  trackAliases,
  publisherPriority,
  client,
  transportState,
  sender = sendVideoObject
}: VideoChunkSendOptions): Promise<{ needsKeyframe: boolean }> {
  if (!trackAliases.length) {
    return { needsKeyframe: false }
  }

  const subgroupId = Number((metadata as { svc?: { temporalLayerId?: number } } | undefined)?.svc?.temporalLayerId ?? 0)
  transportState.ensureVideoSubgroup(subgroupId)

  const shouldIncludeCodec = trackAliases.some((alias) => transportState.shouldSendVideoCodec(alias))
  const decoderConfig = metadata?.decoderConfig as any
  const configBytes: Uint8Array | undefined = decoderConfig?.description && new Uint8Array(decoderConfig.description)
  const includeConfig = chunk.type === 'key' || shouldIncludeCodec
  const payload = new Uint8Array(chunk.byteLength)
  chunk.copyTo(payload)
  const resolvedCaptureTimestampMicros =
    typeof captureTimestampMicros === 'number' && Number.isFinite(captureTimestampMicros)
      ? Math.round(captureTimestampMicros)
      : monotonicUnixMicros()
  const locHeader = buildLocHeader({
    captureTimestampMicros: resolvedCaptureTimestampMicros,
    videoConfig: includeConfig ? configBytes : undefined
  })

  if (chunk.type === 'key') {
    await endVideoGroup(client, trackAliases, transportState)
    transportState.advanceVideoGroup()
    // Send the keyframe to every alias and mark each one as having received a keyframe.
    for (const alias of trackAliases) {
      for (const subgroup of transportState.listVideoSubgroups()) {
        await client.sendSubgroupHeader(alias, transportState.getVideoGroupId(), BigInt(subgroup), publisherPriority)
        transportState.markVideoHeaderSent(alias, subgroup)
      }
    }

    for (const alias of trackAliases) {
      await sender(
        alias,
        transportState.getVideoGroupId(),
        BigInt(subgroupId),
        transportState.getVideoObjectNumber(),
        payload,
        client,
        locHeader
      )
      if (includeConfig && configBytes) {
        transportState.markVideoCodecSent(alias)
      }
      transportState.markVideoKeyframeDelivered(alias)
    }

    transportState.incrementVideoObjectNumber()
    return { needsKeyframe: false }
  }

  // Delta chunk: only send to aliases that already received a keyframe.
  // Pending aliases cannot decode a delta, so skip them and signal that a
  // keyframe is needed so the caller can force one from the encoder.
  const delivered = trackAliases.filter((alias) => transportState.hasVideoKeyframeDelivered(alias))
  const pending = trackAliases.filter((alias) => !transportState.hasVideoKeyframeDelivered(alias))

  for (const alias of delivered) {
    if (!transportState.hasVideoHeaderSent(alias, subgroupId)) {
      await client.sendSubgroupHeader(alias, transportState.getVideoGroupId(), BigInt(subgroupId), publisherPriority)
      transportState.markVideoHeaderSent(alias, subgroupId)
    }
  }

  for (const alias of delivered) {
    await sender(
      alias,
      transportState.getVideoGroupId(),
      BigInt(subgroupId),
      transportState.getVideoObjectNumber(),
      payload,
      client,
      locHeader
    )
    if (includeConfig && configBytes) {
      transportState.markVideoCodecSent(alias)
    }
  }

  // Object number advances once per chunk regardless of how many aliases were
  // sent to; the number is shared across all aliases in the same group.
  transportState.incrementVideoObjectNumber()
  return { needsKeyframe: pending.length > 0 }
}

async function sendVideoObject(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectNumber: bigint,
  payload: Uint8Array,
  client: MOQTClient,
  locHeader?: LocHeader
): Promise<void> {
  await client.sendSubgroupObject(trackAlias, groupId, subgroupId, objectNumber, undefined, payload, locHeader)
}

/// An alias with subgroup 0 open received every object of the group from its
/// keyframe on, so the next object number ends the group for all of them.
async function endVideoGroup(
  client: MOQTClient,
  trackAliases: bigint[],
  transportState: MediaTransportState
): Promise<void> {
  const groupId = transportState.getVideoGroupId()
  const endObjectNumber = transportState.getVideoObjectNumber()
  for (const alias of trackAliases) {
    if (!transportState.hasVideoHeaderSent(alias, 0)) {
      continue
    }
    try {
      await client.sendSubgroupObject(
        alias,
        groupId,
        0n,
        endObjectNumber,
        OBJECT_STATUS_END_OF_GROUP,
        new Uint8Array(0),
        undefined
      )
    } catch (error) {
      // The new group's frames must not wait on a stream that failed to close.
      console.warn('[videoTransport] EndOfGroup send failed', { alias: alias.toString(), error })
    }
  }
}
