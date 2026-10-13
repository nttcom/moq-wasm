import type { SubgroupObjectMessage } from '../../pkg/moqt'
import type { LocHeader } from './loc'
import type { DeserializedChunk } from './chunk'

export type SubgroupObject = Pick<
  SubgroupObjectMessage,
  'subgroupId' | 'objectId' | 'objectPayloadLength' | 'objectPayload' | 'objectStatus'
>

export type SubgroupObjectWithLoc = SubgroupObject & {
  locHeader?: LocHeader
}

export type SubgroupWorkerMessage = {
  type: 'object'
  groupId: bigint
  subgroupStreamObject: SubgroupObjectWithLoc
}

export type JitterBufferSubgroupObject = SubgroupObjectWithLoc & {
  cachedChunk: DeserializedChunk
}
