import type { SubgroupWorkerMessage } from './jitterBufferTypes'

export type DecoderWorkerObjectSource = Pick<
  SubgroupWorkerMessage['subgroupStreamObject'],
  'subgroupId' | 'objectIdDelta' | 'objectPayloadLength' | 'objectPayload' | 'objectStatus' | 'locHeader'
>

export function postSubgroupObjectToWorker(worker: Worker, groupId: bigint, message: DecoderWorkerObjectSource): void {
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
