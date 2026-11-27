import type {
  MoqFetch,
  MoqGroup,
  MoqLocation,
  MoqSession,
  MoqSubscribeGroupStartOptions,
  MoqSubscribeOptions,
  MoqSubscription,
  PublishDone,
  ReceivedObject,
  StreamResetCode,
  SubscriptionEvent
} from '../../../core/session'
import { StreamResetError } from '../../../core/session'
import { TIMESTAMP_ID } from '../../internal/loc'
import { UNOBSERVED_DELIVERIES } from '../deliveryObserver'
import type { TrackContext } from '../trackContext'

export type SubscribeCall = {
  method: 'subscribe' | 'subscribeCurrentGroup' | 'subscribeGroupStart'
  track: string
  options?: MoqSubscribeOptions
  newGroupAfter?: number
}

/// Every SUBSCRIBE answers with `largest` and delivers `objects`; a FETCH
/// delivers the objects `fetchable` lists for its track in its group range and
/// is recorded in `fetches`, such as `video 100-104`.
/// `drained` resolves once a reader has taken them all. `updates` records each
/// UNSUBSCRIBE and SUBSCRIBE_UPDATE sent on a subscription, such as
/// `close video`, `setForward audio true`, `setPriority video 7` or
/// `requestNewGroup video 7`;
/// `failedClose` makes each
/// UNSUBSCRIBE fail. `publishDone` ends the newest subscription to a track
/// with PUBLISH_DONE. `resets` maps a group ID to the code its stream is
/// reset with after its objects.
export class FakeSession {
  readonly calls: SubscribeCall[] = []
  readonly updates: string[] = []
  readonly fetches: string[] = []
  private readonly endings = new Map<string, (done: PublishDone) => void>()
  failedClose: Error | undefined
  drained: Promise<void> = Promise.resolve()
  resets = new Map<bigint, StreamResetCode>()

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

  async subscribeCurrentGroup(
    _namespace: string[],
    track: string,
    options: MoqSubscribeGroupStartOptions = {}
  ): Promise<MoqSubscription> {
    this.calls.push({ method: 'subscribeCurrentGroup', track, options })
    return this.subscription(track)
  }

  async subscribeGroupStart(
    _namespace: string[],
    track: string,
    newGroupAfter: number,
    options: MoqSubscribeGroupStartOptions = {}
  ): Promise<MoqSubscription> {
    this.calls.push({ method: 'subscribeGroupStart', track, newGroupAfter, options })
    return this.subscription(track)
  }

  async fetch(_namespace: string[], track: string, start: MoqLocation, end: MoqLocation): Promise<MoqFetch> {
    this.fetches.push(`${track} ${start.groupId}-${end.groupId}`)
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
    const resets = this.resets
    const groupIds = [...new Set(objects.map((object) => object.groupId))]
    return {
      largest: this.largest,
      done,
      async *[Symbol.asyncIterator]() {
        yield* objects
        drain()
      },
      async *events(): AsyncGenerator<SubscriptionEvent> {
        for (const groupId of groupIds) {
          for (const object of objects.filter((object) => object.groupId === groupId)) {
            yield { kind: 'object', object }
          }
          const code = resets.get(groupId)
          if (code) {
            yield { kind: 'reset', groupId, code }
          }
        }
        drain()
      },
      async *groups() {
        for (const [index, groupId] of groupIds.entries()) {
          const group = objects.filter((object) => object.groupId === groupId)
          const last = index === groupIds.length - 1
          yield {
            groupId,
            subgroupId: 0n,
            [Symbol.asyncIterator]: () => iterate(group, resets.get(groupId), last ? drain : () => {})
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
      },
      setPriority: async (priority: number) => {
        this.updates.push(`setPriority ${track} ${priority}`)
      },
      requestNewGroup: async (largestGroup?: bigint) => {
        this.updates.push(`requestNewGroup ${track} ${largestGroup}`)
        return true
      }
    } as unknown as MoqSubscription
  }
}

async function* iterate(
  objects: ReceivedObject[],
  reset: StreamResetCode | undefined,
  onEnd: () => void
): AsyncGenerator<ReceivedObject> {
  yield* objects
  onEnd()
  if (reset) {
    throw new StreamResetError(reset, `reset with ${reset}`)
  }
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
