import type {
  MoqLocation,
  MoqNamespaceWatch,
  MoqSession,
  MoqSubscribeGroupStartOptions,
  MoqSubscription,
  NamespaceEvent,
  PublishDone,
  ReceivedObject
} from '../../../core/session'
import { RequestError, RequestErrorCode } from '../../../core/requestError'

/// One SUBSCRIBE the session answered: `deliver` hands its reader an object,
/// `end` ends it with PUBLISH_DONE, or with `undefined` as a session that ends
/// without one, after which its reader reads nothing more. After `close`, the
/// reader still takes the objects delivered so far, as a read pending at
/// UNSUBSCRIBE can outlive it (`MoqSubscription.close`).
export type FakeTextSubscription = {
  namespace: string[]
  track: string
  options: MoqSubscribeGroupStartOptions
  closed: boolean
  deliver(groupId: bigint, text: string, objectId?: bigint): void
  end(done: PublishDone | undefined): void
}

/// One SUBSCRIBE_NAMESPACE the session answered: `announce` and `withdraw`
/// hand its reader a namespace event.
export type FakeNamespaceWatch = {
  prefix: string[]
  closed: boolean
  announce(namespace: string[]): void
  withdraw(namespace: string[]): void
}

/// A session whose SUBSCRIBEs stay open until the test ends them; while
/// `absent`, the relay answers every SUBSCRIBE with TRACK_DOES_NOT_EXIST.
/// Every SUBSCRIBE answers with `largest`.
export class FakeTextTrackSession {
  readonly subscriptions: FakeTextSubscription[] = []
  readonly watches: FakeNamespaceWatch[] = []
  absent = false
  largest: MoqLocation | undefined

  get session(): MoqSession {
    return this as unknown as MoqSession
  }

  get isOpen(): boolean {
    return true
  }

  async subscribeCurrentGroup(
    namespace: string[],
    track: string,
    options: MoqSubscribeGroupStartOptions = {}
  ): Promise<MoqSubscription> {
    if (this.absent) {
      throw new RequestError('SUBSCRIBE_ERROR', {
        errorCode: RequestErrorCode.TrackDoesNotExist,
        reasonPhrase: 'track does not exist'
      })
    }
    const queued: ReceivedObject[] = []
    let wake = () => {}
    let ended = false
    let endDone = (_done: PublishDone | undefined) => {}
    const done = new Promise<PublishDone | undefined>((resolve) => {
      endDone = resolve
    })
    const subscription: FakeTextSubscription = {
      namespace,
      track,
      options,
      closed: false,
      deliver: (groupId, text, objectId = 0n) => {
        queued.push(textObject(groupId, objectId, text))
        wake()
      },
      end: (published) => {
        ended = true
        endDone(published)
        wake()
      }
    }
    this.subscriptions.push(subscription)
    return {
      largest: this.largest,
      done,
      async *[Symbol.asyncIterator]() {
        while (!ended) {
          const object = queued.shift()
          if (object) {
            yield object
          } else if (subscription.closed) {
            return
          } else {
            await new Promise<void>((resolve) => {
              wake = resolve
            })
          }
        }
      },
      close: async () => {
        subscription.closed = true
        wake()
      }
    } as unknown as MoqSubscription
  }

  async watchNamespaces(prefix: string[]): Promise<MoqNamespaceWatch> {
    const queued: NamespaceEvent[] = []
    let wake = () => {}
    const push = (event: NamespaceEvent) => {
      queued.push(event)
      wake()
    }
    const watch: FakeNamespaceWatch = {
      prefix,
      closed: false,
      announce: (namespace) => push({ kind: 'announced', namespace }),
      withdraw: (namespace) => push({ kind: 'withdrawn', namespace })
    }
    this.watches.push(watch)
    return {
      async *[Symbol.asyncIterator]() {
        while (!watch.closed) {
          const event = queued.shift()
          if (event) {
            yield event
          } else {
            await new Promise<void>((resolve) => {
              wake = resolve
            })
          }
        }
      },
      close: async () => {
        watch.closed = true
        wake()
      }
    } as unknown as MoqNamespaceWatch
  }

  open(track: string): FakeTextSubscription[] {
    return this.subscriptions.filter((subscription) => subscription.track === track && !subscription.closed)
  }
}

function textObject(groupId: bigint, objectId: bigint, text: string): ReceivedObject {
  const objectPayload = new TextEncoder().encode(text)
  return {
    groupId,
    subgroupId: 0n,
    objectId,
    objectStatus: undefined,
    objectPayload,
    objectPayloadLength: objectPayload.byteLength,
    locHeader: []
  }
}
