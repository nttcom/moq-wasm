import type {
  MoqGroup,
  MoqLocation,
  MoqSession,
  MoqSubscribeOptions,
  MoqSubscription,
  ReceivedObject
} from '../../../core/session'
import { UNOBSERVED_DELIVERIES } from '../deliveryObserver'
import type { TrackContext } from '../trackContext'

export type SubscribeCall = {
  method: 'subscribe' | 'subscribeCurrentGroup'
  track: string
  options?: MoqSubscribeOptions
}

/// Every SUBSCRIBE answers with `largest` and delivers `objects`.
/// `drained` resolves once a reader has taken them all.
export class FakeSession {
  readonly calls: SubscribeCall[] = []
  drained: Promise<void> = Promise.resolve()

  constructor(
    private readonly largest: MoqLocation | undefined,
    private readonly objects: ReceivedObject[]
  ) {}

  context(): TrackContext {
    return {
      session: this as unknown as MoqSession,
      namespace: ['room'],
      observer: UNOBSERVED_DELIVERIES,
      log: () => {}
    }
  }

  get isOpen(): boolean {
    return true
  }

  async subscribe(_namespace: string[], track: string, options: MoqSubscribeOptions = {}): Promise<MoqSubscription> {
    this.calls.push({ method: 'subscribe', track, options })
    return this.subscription()
  }

  async subscribeCurrentGroup(_namespace: string[], track: string): Promise<MoqSubscription> {
    this.calls.push({ method: 'subscribeCurrentGroup', track })
    return this.subscription()
  }

  private subscription(): MoqSubscription {
    let drain = () => {}
    this.drained = new Promise((resolve) => {
      drain = resolve
    })
    const objects = this.objects
    const groupIds = [...new Set(objects.map((object) => object.groupId))]
    return {
      largest: this.largest,
      done: new Promise(() => {}),
      async *[Symbol.asyncIterator]() {
        yield* objects
        drain()
      },
      async *groups() {
        for (const [index, groupId] of groupIds.entries()) {
          const group = objects.filter((object) => object.groupId === groupId)
          const last = index === groupIds.length - 1
          yield {
            groupId,
            subgroupId: 0n,
            [Symbol.asyncIterator]: () => iterate(group, last ? drain : () => {})
          } as unknown as MoqGroup
        }
      },
      close: async () => {},
      setForward: async () => {}
    } as unknown as MoqSubscription
  }
}

async function* iterate(objects: ReceivedObject[], onEnd: () => void): AsyncGenerator<ReceivedObject> {
  yield* objects
  onEnd()
}

export function receivedObject(groupId: bigint, objectId: bigint): ReceivedObject {
  return {
    groupId,
    subgroupId: 0n,
    objectId,
    objectStatus: undefined,
    objectPayload: new Uint8Array([1]),
    objectPayloadLength: 1,
    locHeader: []
  }
}
