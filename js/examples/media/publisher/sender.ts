import { MOQTClient } from '../../../pkg/moqt_client_sample'

export async function sendVideoObjectMessage(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedVideoChunk,
  metadata: EncodedVideoChunkMetadata | undefined,
  client: MOQTClient
) {
  // `EncodedVideoChunk` のデータを Uint8Array に変換
  const chunkArray = new Uint8Array(chunk.byteLength)
  chunk.copyTo(chunkArray)

  const chunkData = {
    type: chunk.type,
    timestamp: chunk.timestamp,
    duration: chunk.duration,
    byteLength: chunk.byteLength,
    data: Array.from(chunkArray),
    decoderConfig: {
      codec: metadata?.decoderConfig?.codec,
      codedHeight: metadata?.decoderConfig?.codedHeight,
      codedWidth: metadata?.decoderConfig?.codedWidth,
      colorSpace: metadata?.decoderConfig?.colorSpace,
      description: metadata?.decoderConfig?.description,
      displayAspectHeight: metadata?.decoderConfig?.displayAspectHeight,
      displayAspectWidth: metadata?.decoderConfig?.displayAspectWidth,
      hardwareAcceleration: metadata?.decoderConfig?.hardwareAcceleration,
      optimizeForLatency: metadata?.decoderConfig?.optimizeForLatency
    },
    temporalLayer: metadata?.temporalLayerId
  }

  const encoder = new TextEncoder()
  const jsonString = JSON.stringify({ chunk: chunkData })
  const objectPayload = encoder.encode(jsonString)

  // TODO: change groupId When keyframe is created
  await client.sendSubgroupStreamObject(BigInt(trackAlias), groupId, subgroupId, objectId, objectPayload)
}

export async function sendAudioObjectMessage(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedAudioChunk,
  metadata: EncodedAudioChunkMetadata | undefined,
  client: MOQTClient
) {
  // `EncodedAudioChunk` のデータを Uint8Array に変換
  const chunkArray = new Uint8Array(chunk.byteLength)
  chunk.copyTo(chunkArray)

  const chunkData = {
    type: chunk.type,
    timestamp: chunk.timestamp,
    duration: chunk.duration,
    byteLength: chunk.byteLength,
    data: Array.from(chunkArray),
    decoderConfig: {
      codec: metadata?.decoderConfig?.codec,
      numberOfChannels: metadata?.decoderConfig?.numberOfChannels,
      sampleRate: metadata?.decoderConfig?.sampleRate
    }
  }

  const encoder = new TextEncoder()
  const jsonString = JSON.stringify({ chunk: chunkData })
  const objectPayload = encoder.encode(jsonString)
  await client.sendSubgroupStreamObject(BigInt(trackAlias), groupId, subgroupId, objectId, objectPayload)
}
