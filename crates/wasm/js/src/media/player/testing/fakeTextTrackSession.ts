import type { MoqSession, MoqSubscription, PublishDone, ReceivedObject } from '../../../core/session'
import { RequestError, RequestErrorCode } from '../../../core/requestError'

/// One SUBSCRIBE the session answered: `deliver` hands its reader an object,
/// `end` ends it with PUBLISH_DONE.
export type FakeTextSubscription = {
  track: string
  closed: boolean
  deliver(groupId: bigint, text: string): void
  end(done: PublishDone): void
}

/// A session whose SUBSCRIBEs stay open until the test ends them; while
/// `absent`, the relay answers every SUBSCRIBE with TRACK_DOES_NOT_EXIST.
export class FakeTextTrackSession {
  readonly subscriptions: FakeTextSubscription[] = []
  absent = false

  get session(): MoqSession {
    return this as unknown as MoqSession
  }

  get isOpen(): boolean {
    return true
  }

  async subscribeCurrentGroup(_namespace: string[], track: string): Promise<MoqSubscription> {
    if (this.absent) {
      throw new RequestError('SUBSCRIBE_ERROR', {
        errorCode: RequestErrorCode.TrackDoesNotExist,
        reasonPhrase: 'track does not exist'
      })
    }
    const queued: ReceivedObject[] = []
    let wake = () => {}
    let endDone = (_done: PublishDone) => {}
    const done = new Promise<PublishDone | undefined>((resolve) => {
      endDone = resolve
    })
    const subscription: FakeTextSubscription = {
      track,
      closed: false,
      deliver: (groupId, text) => {
        queued.push(textObject(groupId, text))
        wake()
      },
      end: (published) => endDone(published)
    }
    this.subscriptions.push(subscription)
    return {
      largest: undefined,
      done,
      async *[Symbol.asyncIterator]() {
        while (!subscription.closed) {
          const object = queued.shift()
          if (object) {
            yield object
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

  open(track: string): FakeTextSubscription[] {
    return this.subscriptions.filter((subscription) => subscription.track === track && !subscription.closed)
  }
}

function textObject(groupId: bigint, text: string): ReceivedObject {
  const objectPayload = new TextEncoder().encode(text)
  return {
    groupId,
    subgroupId: 0n,
    objectId: 0n,
    objectStatus: undefined,
    objectPayload,
    objectPayloadLength: objectPayload.byteLength,
    locHeader: []
  }
}
