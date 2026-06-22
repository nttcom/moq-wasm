import type { MOQTClient } from '../../pkg/moqt_client_wasm'
import { MediaTransportState } from './transportState'
import { serializeChunk } from './chunk'
import { buildLocHeader, bytesToBase64, arrayBufferToUint8Array, type LocHeader } from './loc'
import { monotonicUnixMicros } from './clock'

const H264_BOOTSTRAP_FRAME_INTERVAL_US = 33_333

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
}: VideoChunkSendOptions): Promise<{ needsKeyframe: boolean }> {
  if (!trackAliases.length) {
    return { needsKeyframe: false }
  }

  const subgroupId = Number((metadata as { svc?: { temporalLayerId?: number } } | undefined)?.svc?.temporalLayerId ?? 0)
  transportState.ensureVideoSubgroup(subgroupId)

  const shouldIncludeCodec = trackAliases.some((alias) => transportState.shouldSendVideoCodec(alias))
  const decoderConfig = metadata?.decoderConfig as any
  const codec = typeof decoderConfig?.codec === 'string' ? decoderConfig.codec : undefined
  const configBytes = arrayBufferToUint8Array(decoderConfig?.description)
  const includeConfig = chunk.type === 'key' || shouldIncludeCodec
  const payload = serializeChunk(chunk, {
    codec,
    timestamp: normalizeVideoChunkTimestamp(chunk, codec),
    descriptionBase64: includeConfig && configBytes ? bytesToBase64(configBytes) : undefined,
    avcFormat: codec?.startsWith('avc') ? 'annexb' : undefined
  })
  const resolvedCaptureTimestampMicros =
    typeof captureTimestampMicros === 'number' && Number.isFinite(captureTimestampMicros)
      ? Math.round(captureTimestampMicros)
      : monotonicUnixMicros()
  const locHeader = buildLocHeader({
    captureTimestampMicros: resolvedCaptureTimestampMicros,
    videoConfig: includeConfig ? configBytes : undefined
  })

  // EndOfGroup for the previous group is sent per alias by
  // mediaPublisher.sendVideoObjectForTrackContext when it detects the group
  // change; sending it here as well would race on the same object number.
  if (chunk.type === 'key' || transportState.getVideoGroupId() < 0n) {
    transportState.advanceVideoGroup()
  }

  if (chunk.type === 'key') {
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

function normalizeVideoChunkTimestamp(chunk: EncodedVideoChunk, codec: string | undefined): number {
  if (codec?.startsWith('avc') && chunk.type === 'key' && chunk.timestamp === 0) {
    return H264_BOOTSTRAP_FRAME_INTERVAL_US
  }
  return chunk.timestamp
}
