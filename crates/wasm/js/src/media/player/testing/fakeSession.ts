import type {
  MoqFetch,
  MoqGroup,
  MoqLocation,
  MoqSession,
  MoqSubscribeOptions,
  MoqSubscription,
  PublishDone,
  ReceivedObject
} from '../../../core/session'
import { TIMESTAMP_ID } from '../../internal/loc'
import { UNOBSERVED_DELIVERIES } from '../deliveryObserver'
import type { TrackContext } from '../trackContext'

export type SubscribeCall = {
  method: 'subscribe' | 'subscribeCurrentGroup'
  track: string
  options?: MoqSubscribeOptions
}

/// Every SUBSCRIBE answers with `largest` and delivers `objects`; a FETCH
/// delivers the objects `fetchable` lists for its track in its group range.
/// `drained` resolves once a reader has taken them all. `updates` records each
/// UNSUBSCRIBE and SUBSCRIBE_UPDATE sent on a subscription, such as
/// `close video` or `setForward audio true`; `failedClose` makes each
/// UNSUBSCRIBE fail. `publishDone` ends the newest subscription to a track
/// with PUBLISH_DONE.
export class FakeSession {
  readonly calls: SubscribeCall[] = []
  readonly updates: string[] = []
  private readonly endings = new Map<string, (done: PublishDone) => void>()
  failedClose: Error | undefined
  drained: Promise<void> = Promise.resolve()

  constructor(
    private readonly largest: MoqLocation | undefined,
    private readonly objects: ReceivedObject[],
    private readonly fetchable: Record<string, ReceivedObject[]> = {}
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
    return this.subscription(track)
  }

  async subscribeCurrentGroup(_namespace: string[], track: string): Promise<MoqSubscription> {
    this.calls.push({ method: 'subscribeCurrentGroup', track })
    return this.subscription(track)
  }

  async fetch(_namespace: string[], track: string, start: MoqLocation, end: MoqLocation): Promise<MoqFetch> {
    const objects = (this.fetchable[track] ?? []).filter(
      ({ groupId }) => groupId >= start.groupId && groupId <= end.groupId
    )
    return {
      async *[Symbol.asyncIterator]() {
        yield* objects
      }
    } as unknown as MoqFetch
  }

  publishDone(track: string, done: PublishDone): void {
    this.endings.get(track)?.(done)
  }

  private subscription(track: string): MoqSubscription {
    const done = new Promise<PublishDone>((resolve) => this.endings.set(track, resolve))
    let drain = () => {}
    this.drained = new Promise((resolve) => {
      drain = resolve
    })
    const objects = this.objects
    const groupIds = [...new Set(objects.map((object) => object.groupId))]
    return {
      largest: this.largest,
      done,
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
      close: async () => {
        this.updates.push(`close ${track}`)
        if (this.failedClose) {
          throw this.failedClose
        }
      },
      setForward: async (forward: boolean) => {
        this.updates.push(`setForward ${track} ${forward}`)
      }
    } as unknown as MoqSubscription
  }
}

async function* iterate(objects: ReceivedObject[], onEnd: () => void): AsyncGenerator<ReceivedObject> {
  yield* objects
  onEnd()
}

export function receivedObject(groupId: bigint, objectId: bigint, captureMicros?: number): ReceivedObject {
  return {
    groupId,
    subgroupId: 0n,
    objectId,
    objectStatus: undefined,
    objectPayload: new Uint8Array([1]),
    objectPayloadLength: 1,
    locHeader: captureMicros === undefined ? [] : [{ id: TIMESTAMP_ID, value: { varint: captureMicros } }]
  }
}
