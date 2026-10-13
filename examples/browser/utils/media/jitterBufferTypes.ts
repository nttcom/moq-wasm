import type { SubgroupObjectMessage } from '../../pkg/moqt'
import type { LocHeader } from './loc'

export type SubgroupObjectWithLoc = Pick<
  SubgroupObjectMessage,
  'subgroupId' | 'objectId' | 'objectPayloadLength' | 'objectPayload' | 'objectStatus'
> & {
  locHeader?: LocHeader
}

export type SubgroupWorkerMessage = {
  type: 'object'
  groupId: bigint
  subgroupStreamObject: SubgroupObjectWithLoc
}
