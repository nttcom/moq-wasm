import type { ReceivedObject } from '../../core/session'
import type { LocHeader } from './loc'

export type SubgroupObjectWithLoc = Pick<
  ReceivedObject,
  'subgroupId' | 'objectId' | 'objectPayloadLength' | 'objectPayload' | 'objectStatus'
> & {
  locHeader?: LocHeader
}

export type SubgroupWorkerMessage = {
  type: 'object'
  groupId: bigint
  subgroupStreamObject: SubgroupObjectWithLoc
  /// The catalog codec of the track the object came from.
  codec?: string
}
