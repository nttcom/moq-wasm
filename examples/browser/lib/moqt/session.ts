import type * as Moqt from '../../pkg/moqt'
import { loadMoqt } from './loadMoqt'
import { RequestError } from './moqtClient'

/// An object as the player reads it, copied out of the wasm memory.
export type ReceivedObject = {
  readonly groupId: bigint
  readonly subgroupId: bigint
  readonly objectId: bigint
  /// The draft-14 Object Status of an object without a payload.
  readonly objectStatus: number | undefined
  readonly objectPayload: Uint8Array
  readonly objectPayloadLength: number
  readonly locHeader: any
}

export type MoqLocation = { readonly groupId: bigint; readonly objectId: bigint }

export type PublishDone = { readonly code: bigint; readonly reason: string }

export type CloseInfo = { readonly code: number | undefined; readonly reason: string }

export type MoqConnectOptions = {
  /// Called before each CLIENT_SETUP, so a short-lived token can be fetched
  /// for every connection, a reconnect or a GOAWAY move included.
  token?: string | (() => Promise<string>)
}

export type MoqSubscribeOptions = {
  filter?: 'largestObject' | 'nextGroupStart'
  forward?: boolean
}

/// The parts of the wasm classes this module uses, so tests can stand in for
/// them.
type WasmSession = Pick<
  Moqt.Session,
  'closed' | 'subscribe' | 'subscribeCurrentGroup' | 'fetch' | 'trackStatus' | 'close'
>
type WasmSubscription = Pick<
  Moqt.Subscription,
  'largest' | 'done' | 'next' | 'nextGroup' | 'setForward' | 'endBeforeGroup' | 'close' | 'free'
>
type WasmGroup = Pick<Moqt.Group, 'groupId' | 'subgroupId' | 'next' | 'free'>
type WasmObject = Pick<
  Moqt.MoqObject,
  'groupId' | 'subgroupId' | 'objectId' | 'status' | 'payload' | 'locHeader' | 'free'
>
type WasmStream = Pick<Moqt.FetchStream, 'next' | 'free'>

/// A MoQT session to a relay. Each request resolves to a handle; a request
/// the relay rejects throws a `RequestError` with the draft-14 code.
export class MoqSession {
  readonly closed: Promise<CloseInfo>
  private open = true

  constructor(private readonly inner: WasmSession) {
    this.closed = inner.closed.then((close: Moqt.CloseInfo) => {
      this.open = false
      const info = { code: close.code, reason: close.reason }
      close.free()
      return info
    })
  }

  static async connect(url: string, options: MoqConnectOptions = {}): Promise<MoqSession> {
    const moqt = await loadMoqt()
    return new MoqSession(await moqt.Session.connect(url, options.token))
  }

  get isOpen(): boolean {
    return this.open
  }

  async subscribe(namespace: string[], track: string, options: MoqSubscribeOptions = {}): Promise<MoqSubscription> {
    const subscription = await request('SUBSCRIBE', () =>
      this.inner.subscribe(namespace, track, options.filter, options.forward)
    )
    return new MoqSubscription(subscription)
  }

  /// SUBSCRIBE with a joining FETCH: the objects of the group the largest
  /// object is in come first.
  async subscribeCurrentGroup(namespace: string[], track: string): Promise<MoqSubscription> {
    const subscription = await request('SUBSCRIBE', () => this.inner.subscribeCurrentGroup(namespace, track))
    return new MoqSubscription(subscription)
  }

  /// `end` is the End Location of draft-14 §9.16.1: the last object plus 1,
  /// with Object 0 meaning the whole group.
  async fetch(namespace: string[], track: string, start: MoqLocation, end: MoqLocation): Promise<MoqFetch> {
    const stream = await request('FETCH', () =>
      this.inner.fetch(namespace, track, start.groupId, start.objectId, end.groupId, end.objectId)
    )
    return new MoqFetch(stream)
  }

  /// The track's largest location, `undefined` while it has none.
  async trackStatus(namespace: string[], track: string): Promise<MoqLocation | undefined> {
    const largest = await request('TRACK_STATUS', () => this.inner.trackStatus(namespace, track))
    return largest && location(largest)
  }

  close(): void {
    this.open = false
    this.inner.close()
  }
}

export class MoqSubscription implements AsyncIterable<ReceivedObject> {
  readonly largest: MoqLocation | undefined
  /// Resolves once the publisher ends the subscription (PUBLISH_DONE), or to
  /// `undefined` once the session ends without one.
  readonly done: Promise<PublishDone | undefined>
  /// The wasm object is freed once it is closed and no read awaits it: a read
  /// in flight borrows it.
  private reads = 0
  private closed = false
  private freed = false

  constructor(private readonly inner: WasmSubscription) {
    const largest = inner.largest
    this.largest = largest && location(largest)
    this.done = inner.done.then((done: Moqt.PublishDoneInfo | undefined) => {
      if (!done) {
        return undefined
      }
      const info = { code: done.code, reason: done.reason }
      done.free()
      return info
    })
  }

  /// Every object, status objects excepted; ordered within a subgroup only.
  async *[Symbol.asyncIterator](): AsyncIterator<ReceivedObject> {
    for (;;) {
      const object = await this.read(() => this.inner.next())
      if (!object) {
        return
      }
      yield received(object)
    }
  }

  /// Each subgroup stream, with its objects in order and its status objects.
  async *groups(): AsyncIterable<MoqGroup> {
    for (;;) {
      const group = await this.read(() => this.inner.nextGroup())
      if (!group) {
        return
      }
      yield new MoqGroup(group)
    }
  }

  setForward(forward: boolean): Promise<void> {
    return this.inner.setForward(forward)
  }

  /// Delivers the groups before `groupId`, after which the publisher ends the
  /// subscription.
  endBeforeGroup(groupId: bigint, forward: boolean): Promise<void> {
    return this.inner.endBeforeGroup(groupId, forward)
  }

  /// UNSUBSCRIBE. Reads end once the publisher stops the streams.
  async close(): Promise<void> {
    if (this.closed) {
      return
    }
    try {
      await this.inner.close()
    } finally {
      this.closed = true
      this.freeIfIdle()
    }
  }

  private async read<T>(next: () => Promise<T>): Promise<T | undefined> {
    if (this.freed) {
      return undefined
    }
    this.reads += 1
    try {
      return await next()
    } finally {
      this.reads -= 1
      this.freeIfIdle()
    }
  }

  private freeIfIdle(): void {
    if (this.closed && this.reads === 0 && !this.freed) {
      this.freed = true
      this.inner.free()
    }
  }
}

export class MoqGroup implements AsyncIterable<ReceivedObject> {
  readonly groupId: bigint
  readonly subgroupId: bigint

  constructor(private readonly inner: WasmGroup) {
    this.groupId = inner.groupId
    this.subgroupId = inner.subgroupId
  }

  /// Ends once the stream finishes; throws when it was reset.
  async *[Symbol.asyncIterator](): AsyncIterator<ReceivedObject> {
    try {
      for (;;) {
        const object = await this.inner.next()
        if (!object) {
          return
        }
        yield received(object)
      }
    } finally {
      this.inner.free()
    }
  }
}

/// The objects of one FETCH response; throws when the publisher resets it.
export class MoqFetch implements AsyncIterable<ReceivedObject> {
  constructor(private readonly inner: WasmStream) {}

  async *[Symbol.asyncIterator](): AsyncIterator<ReceivedObject> {
    try {
      for (;;) {
        const object = await this.inner.next()
        if (!object) {
          return
        }
        yield received(object)
      }
    } finally {
      this.inner.free()
    }
  }
}

function received(object: WasmObject): ReceivedObject {
  const objectPayload = object.payload
  const copy = {
    groupId: object.groupId,
    subgroupId: object.subgroupId,
    objectId: object.objectId,
    objectStatus: object.status,
    objectPayload,
    objectPayloadLength: objectPayload.byteLength,
    locHeader: object.locHeader
  }
  object.free()
  return copy
}

function location(location: Moqt.MoqLocation): MoqLocation {
  const copy = { groupId: location.groupId, objectId: location.objectId }
  location.free()
  return copy
}

/// The wasm classes reject with an `Error` whose `code` is the request error
/// code; anything else is passed on as is.
async function request<T>(messageType: string, send: () => Promise<T>): Promise<T> {
  try {
    return await send()
  } catch (error) {
    if (error instanceof Error && typeof (error as { code?: unknown }).code === 'bigint') {
      throw new RequestError(messageType, {
        errorCode: (error as Error & { code: bigint }).code,
        reasonPhrase: error.message
      })
    }
    throw error
  }
}
