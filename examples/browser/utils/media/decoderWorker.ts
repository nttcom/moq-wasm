import type { SubgroupObjectWithLoc } from './jitterBufferTypes'

export function postSubgroupObjectToWorker(worker: Worker, groupId: bigint, message: SubgroupObjectWithLoc): void {
  const payload = new Uint8Array(message.objectPayload)
  worker.postMessage(
    {
      groupId,
      subgroupStreamObject: {
        subgroupId: message.subgroupId,
        objectIdDelta: message.objectIdDelta,
        objectPayloadLength: message.objectPayloadLength,
        objectPayload: payload,
        objectStatus: message.objectStatus,
        locHeader: message.locHeader
      }
    },
    [payload.buffer]
  )
}
