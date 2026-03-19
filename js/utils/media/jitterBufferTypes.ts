import type { SubgroupStreamObjectMessage } from '../../pkg/moqt_client_wasm'
import type { LocHeader } from './loc'
import type { DeserializedChunk } from './chunk'

export type SubgroupObject = Pick<
  SubgroupStreamObjectMessage,
  'objectId' | 'objectPayloadLength' | 'objectPayload' | 'objectStatus'
>

export type SubgroupObjectWithLoc = SubgroupObject & {
  locHeader?: LocHeader
}

export type SubgroupWorkerMessage = {
  groupId: bigint
  subgroupStreamObject: SubgroupObjectWithLoc
}

export type JitterBufferSubgroupObject = SubgroupObjectWithLoc & {
  cachedChunk: DeserializedChunk
  remotePTS: number
  localPTS: number
}
