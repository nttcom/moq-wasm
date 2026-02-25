import type { MOQTClient } from '../../../pkg/moqt_client_wasm'
import { createBitrateLogger } from '../../../utils/media/bitrate'

const chunkDataBitrateLogger = createBitrateLogger((kbps) => {
  console.debug('[CmafSender] bitrate', kbps, 'kbps')
})

export async function sendCmafVideoObject(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  cmafSegment: Uint8Array,
  client: MOQTClient
): Promise<void> {
  console.debug('[CmafSender] sendCmafVideoObject', {
    trackAlias: trackAlias.toString(),
    groupId: groupId.toString(),
    subgroupId: subgroupId.toString(),
    objectId: objectId.toString(),
    byteLength: cmafSegment.byteLength,
  })

  // Prepend 8-byte timestamp (Date.now()) for E2E delay measurement
  const payload = new Uint8Array(8 + cmafSegment.byteLength)
  new DataView(payload.buffer).setBigUint64(0, BigInt(Date.now()))
  payload.set(cmafSegment, 8)

  chunkDataBitrateLogger.addBytes(payload.byteLength)
  await client.sendSubgroupStreamObject(
    BigInt(trackAlias),
    groupId,
    subgroupId,
    objectId,
    undefined,
    payload
  )
}
