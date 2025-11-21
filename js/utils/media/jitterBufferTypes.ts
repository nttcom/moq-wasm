import type { SubgroupStreamObjectMessage } from '../../pkg/moqt_client_sample'
import type { DeserializedChunk } from './chunk'

export type SubgroupObject = Pick<
  SubgroupStreamObjectMessage,
  'objectId' | 'objectPayloadLength' | 'objectPayload' | 'objectStatus'
>

export type SubgroupWorkerMessage = {
  groupId: bigint
  subgroupStreamObject: SubgroupObject
}

export type JitterBufferSubgroupObject = SubgroupObject & {
  cachedChunk: DeserializedChunk
  remotePTS: number
  localPTS: number
}
