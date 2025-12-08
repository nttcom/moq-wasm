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

  const { extraMeta, shouldIncludeCodec } = buildAudioMetadata(trackAliases, transportState, metadata)

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
  }, extraMeta)

  for (const alias of trackAliases) {
    await client.sendSubgroupStreamObject(
      alias,
      transportState.getAudioGroupId(),
      BigInt(subgroupId),
      transportState.getAudioObjectId(),
      undefined,
      payload
    )
    if (shouldIncludeCodec) {
      transportState.markAudioCodecSent(alias)
    }
  }

  transportState.incrementAudioObject()
}

/**
 * Build metadata for the first audio object (codec/config) and indicate whether it should be sent.
 */
function buildAudioMetadata(
  trackAliases: bigint[],
  transportState: MediaTransportState,
  metadata: EncodedAudioChunkMetadata | undefined
) {
  const shouldIncludeCodec = trackAliases.some((alias) => transportState.shouldSendAudioCodec(alias))
  if (!shouldIncludeCodec) {
    return { extraMeta: undefined, shouldIncludeCodec }
  }

  const decoderConfig = metadata?.decoderConfig
  const descriptionBase64 =
    decoderConfig?.description ? bufferToBase64(decoderConfig.description as ArrayBuffer) : undefined

  return {
    shouldIncludeCodec,
    extraMeta: {
      codec: decoderConfig?.codec ?? 'opus',
      sampleRate: decoderConfig?.sampleRate ?? 48000,
      channels: decoderConfig?.numberOfChannels ?? 1,
      descriptionBase64
    }
  }
}

function bufferToBase64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf)
  let binary = ''
  for (let i = 0; i < bytes.byteLength; i++) {
    binary += String.fromCharCode(bytes[i])
  }
  return btoa(binary)
}
