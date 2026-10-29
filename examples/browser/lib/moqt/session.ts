import type * as Moqt from '../../pkg/moqt'
import { loadMoqt } from './loadMoqt'
import { RequestError } from './moqtClient'

/// An object as the player reads it, copied out of the wasm memory.
export type ReceivedObject = {
  readonly groupId: bigint
  readonly subgroupId: bigint
  readonly objectId: bigint
  /// The draft-14 Object Status (§10.2.1.1); `undefined` for an object with a payload.
  readonly objectStatus: number | undefined
  readonly objectPayload: Uint8Array
  readonly objectPayloadLength: number
  readonly locHeader: any
}

export type MoqLocation = { readonly groupId: bigint; readonly objectId: bigint }

export type PublishDone = { readonly code: bigint; readonly reason: string }

export type CloseInfo = { readonly code: number | undefined; readonly reason: string }

export type MoqConnectOptions = {
  /// Called before each CLIENT_SETUP, a reconnect or a GOAWAY move included.
  token?: string | (() => Promise<string>)
}

export type PublishTrackOptions = {
  /// How long the session keeps each object to answer FETCH; 60 s by
  /// default, and 0 keeps only the newest object.
  retentionMs?: number
  /// Sends PUBLISH so the relay takes the track before anyone subscribes;
  /// announce the namespace first.
  publish?: boolean
}

export type MoqSubscribeOptions = {
  filter?: 'largestObject' | 'nextGroupStart'
  forward?: boolean
}

type WasmSession = Pick<
  Moqt.Session,
  | 'closed'
  | 'subscribe'
  | 'subscribeCurrentGroup'
  | 'fetch'
  | 'trackStatus'
  | 'announce'
  | 'publishTrack'
  | 'broadcast'
  | 'close'
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

/// A request the relay rejects throws a `RequestError` with the draft-14 code.
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
    const startedAtMs = performance.now()
    const session = await moqt.Session.connect(url, options.token)
    const elapsedMs = Math.round(performance.now() - startedAtMs)
    console.info(`[moqt] session established over ${session.transport} in ${elapsedMs} ms`)
    return new MoqSession(session)
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

  /// SUBSCRIBE with a joining FETCH (draft-14 §9.16.2): the objects of the group
  /// the largest object is in come first.
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

  /// PUBLISH_NAMESPACE; the returned announcement's `close` withdraws it.
  async announce(namespace: string[]): Promise<Moqt.Announcement> {
    return request('PUBLISH_NAMESPACE', () => this.inner.announce(namespace))
  }

  /// Numbers, caches and sends what the writer writes, and answers the track's
  /// SUBSCRIBE and FETCH.
  publishTrack(namespace: string[], track: string, options: PublishTrackOptions = {}): Promise<Moqt.TrackWriter> {
    return this.inner.publishTrack(namespace, track, options.retentionMs, options.publish)
  }

  /// To publish a MediaStream, use `publish` in `lib/publish`, which feeds
  /// this broadcast.
  broadcast(namespace: string[]): Moqt.MediaBroadcast {
    return this.inner.broadcast(namespace)
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
    yield* objects(() => this.read(() => this.inner.next()))
  }

  /// Each subgroup stream, status objects included.
  async *groups(): AsyncIterable<MoqGroup> {
    for (;;) {
      const group = await this.read(() => this.inner.nextGroup())
      if (!group) {
        return
      }
      const received = new MoqGroup(group)
      console.debug('[moqt] received subgroup header', {
        groupId: received.groupId.toString(),
        subgroupId: received.subgroupId.toString()
      })
      yield received
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
      yield* objects(() => this.inner.next())
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
      yield* objects(() => this.inner.next())
    } finally {
      this.inner.free()
    }
  }
}

async function* objects(next: () => Promise<WasmObject | undefined>): AsyncGenerator<ReceivedObject> {
  for (;;) {
    const object = await next()
    if (!object) {
      return
    }
    yield received(object)
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
