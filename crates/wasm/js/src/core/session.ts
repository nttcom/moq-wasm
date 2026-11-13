import type * as Moqt from '../../pkg/moqt'
import { loadMoqt } from './init'
import { RequestError } from './requestError'
import { bindLogToken, sdkLog } from './log'

const TARGET = 'moq-wasm::core::session'

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

export type MoqVersion = 'draft-14' | 'draft-18'

export type TransportStats = { readonly sentBytes: number; readonly receivedBytes: number }

export type MoqConnectOptions = {
  /// Called before each CLIENT_SETUP, a reconnect or a GOAWAY move included.
  token?: string | (() => Promise<string>)
  /// The drafts offered to the relay, most preferred first; draft-18, then draft-14, when omitted.
  versions?: MoqVersion[]
}

export type PublishTrackOptions = {
  /// How long the session keeps each object to answer FETCH; 60 s by
  /// default, and 0 keeps only the newest object.
  retentionMs?: number
  /// Sends PUBLISH so the relay takes the track before anyone subscribes;
  /// announce the namespace first.
  publish?: boolean
  /// Sends each object in its own datagram (draft-14 §10.3) instead of on
  /// subgroup streams: a lost object is not resent and never holds back the
  /// next one. An object larger than the path's datagram size (about 1 KB is
  /// safe) is dropped. A subscriber reads the track with the async iterator,
  /// not `groups()`.
  datagram?: boolean
}

export type MoqSubscribeOptions = {
  filter?: 'largestObject' | 'nextGroupStart'
  forward?: boolean
  /// The Subscriber Priority (draft-14 §7.2), 128 by default: the relay sends a
  /// lower value first.
  priority?: number
}

/// `withdrawn` follows PUBLISH_NAMESPACE_DONE.
export type NamespaceEvent = { readonly kind: 'announced' | 'withdrawn'; readonly namespace: string[] }

type WasmSession = Pick<
  Moqt.Session,
  | 'closed'
  | 'token'
  | 'transportStats'
  | 'subscribe'
  | 'subscribeCurrentGroup'
  | 'subscribeGroupStart'
  | 'fetch'
  | 'trackStatus'
  | 'watchNamespaces'
  | 'announce'
  | 'publishTrack'
  | 'broadcast'
  | 'mp4Broadcast'
  | 'close'
>
type WasmSubscription = Pick<
  Moqt.Subscription,
  | 'largest'
  | 'done'
  | 'next'
  | 'nextGroup'
  | 'setForward'
  | 'setPriority'
  | 'requestNewGroup'
  | 'endBeforeGroup'
  | 'close'
  | 'free'
>
type WasmGroup = Pick<Moqt.Group, 'groupId' | 'subgroupId' | 'next' | 'free'>
type WasmObject = Pick<
  Moqt.MoqObject,
  'groupId' | 'subgroupId' | 'objectId' | 'status' | 'payload' | 'locHeader' | 'free'
>
type WasmStream = Pick<Moqt.FetchStream, 'next' | 'free'>
type WasmNamespaceWatch = Pick<Moqt.NamespaceWatch, 'next' | 'close' | 'free'>

/// A request the relay rejects throws a `RequestError` with the draft-14 code.
export class MoqSession {
  readonly closed: Promise<CloseInfo>
  private open = true

  constructor(private readonly inner: WasmSession) {
    bindLogToken(this, () => inner.token)
    this.closed = inner.closed.then((close: Moqt.CloseInfo) => {
      this.open = false
      const info = { code: close.code, reason: close.reason }
      close.free()
      return info
    })
  }

  static async connect(url: string, options: MoqConnectOptions = {}): Promise<MoqSession> {
    const calledAtMs = performance.now()
    const moqt = await loadMoqt()
    const startedAtMs = performance.now()
    const session = await moqt.Session.connect(
      url,
      options.token,
      Math.round(startedAtMs - calledAtMs),
      options.versions
    )
    const elapsedMs = Math.round(performance.now() - startedAtMs)
    const moqSession = new MoqSession(session)
    sdkLog(moqSession, 'info', TARGET, 'session established', { transport: session.transport, duration_ms: elapsedMs })
    return moqSession
  }

  get isOpen(): boolean {
    return this.open
  }

  /// Bytes sent and received since the session opened, over every connection
  /// a GOAWAY move or reconnect opened.
  transportStats(): TransportStats {
    const stats = this.inner.transportStats()
    const copy = { sentBytes: stats.sentBytes, receivedBytes: stats.receivedBytes }
    stats.free()
    return copy
  }

  async subscribe(namespace: string[], track: string, options: MoqSubscribeOptions = {}): Promise<MoqSubscription> {
    const subscription = await request('SUBSCRIBE', () =>
      this.inner.subscribe(namespace, track, options.filter, options.forward, options.priority)
    )
    return new MoqSubscription(subscription, this)
  }

  /// SUBSCRIBE with a joining FETCH (draft-14 §9.16.2): the objects of the group
  /// the largest object is in come first.
  async subscribeCurrentGroup(namespace: string[], track: string): Promise<MoqSubscription> {
    const subscription = await request('SUBSCRIBE', () => this.inner.subscribeCurrentGroup(namespace, track))
    return new MoqSubscription(subscription, this)
  }

  /// SUBSCRIBE that reads from the first object of a group (draft-18 §5.1.3.1).
  /// The group the largest object is in comes first, through a joining FETCH,
  /// unless the track starts a group on request and the largest object ID is
  /// `newGroupAfter` or more: then the publisher is asked for a new
  /// group (NEW_GROUP_REQUEST, §10.2.13) and the objects start with it.
  /// `priority` is the Subscriber Priority, as in `subscribe`.
  async subscribeGroupStart(
    namespace: string[],
    track: string,
    newGroupAfter: number,
    priority?: number
  ): Promise<MoqSubscription> {
    const subscription = await request('SUBSCRIBE', () =>
      this.inner.subscribeGroupStart(namespace, track, newGroupAfter, priority)
    )
    return new MoqSubscription(subscription, this)
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

  /// SUBSCRIBE_NAMESPACE on `prefix`; resolves once the relay accepts it.
  async watchNamespaces(prefix: string[]): Promise<MoqNamespaceWatch> {
    const watch = await request('SUBSCRIBE_NAMESPACE', () => this.inner.watchNamespaces(prefix))
    return new MoqNamespaceWatch(watch)
  }

  /// PUBLISH_NAMESPACE; the returned announcement's `close` withdraws it.
  async announce(namespace: string[]): Promise<Moqt.Announcement> {
    return request('PUBLISH_NAMESPACE', () => this.inner.announce(namespace))
  }

  /// Numbers, caches and sends what the writer writes, and answers the track's
  /// SUBSCRIBE and FETCH.
  publishTrack(namespace: string[], track: string, options: PublishTrackOptions = {}): Promise<Moqt.TrackWriter> {
    return this.inner.publishTrack(namespace, track, options.retentionMs, options.publish, options.datagram)
  }

  /// To publish a MediaStream, use `publish` from `moq-wasm/media`, which feeds
  /// this broadcast.
  broadcast(namespace: string[]): Moqt.MediaBroadcast {
    return this.inner.broadcast(namespace)
  }

  /// Throws when `index` has no H.264 video.
  mp4Broadcast(namespace: string[], index: Moqt.Mp4Index): Moqt.Mp4Broadcast {
    return this.inner.mp4Broadcast(namespace, index)
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
  /// A call in flight borrows the wasm object until it settles, so the object
  /// is freed only after the last call.
  private calls = 0
  private closed = false
  private freed = false

  constructor(
    private readonly inner: WasmSubscription,
    private readonly session?: MoqSession
  ) {
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
    yield* objects(() => this.call(() => this.inner.next()))
  }

  /// Each subgroup stream, status objects included.
  async *groups(): AsyncIterable<MoqGroup> {
    for (;;) {
      const group = await this.call(() => this.inner.nextGroup())
      if (!group) {
        return
      }
      const received = new MoqGroup(group)
      sdkLog(this.session, 'debug', TARGET, 'subgroup header received', {
        group_id: received.groupId,
        subgroup_id: received.subgroupId
      })
      yield received
    }
  }

  async setForward(forward: boolean): Promise<void> {
    await this.call(() => this.inner.setForward(forward))
  }

  /// Changes the Subscriber Priority (draft-14 §7.1): the relay sends a lower
  /// value first, and orders what it has not sent yet by the new value.
  async setPriority(priority: number): Promise<void> {
    await this.call(() => this.inner.setPriority(priority))
  }

  /// Asks the publisher to start a new group (NEW_GROUP_REQUEST, draft-18
  /// §10.2.13). `largestGroup` is the largest group ID of the track seen so
  /// far. Resolves to false, without asking, for a track that does not start
  /// groups on request. The relay does not pass on a request that follows
  /// another within 2 seconds.
  async requestNewGroup(largestGroup?: bigint): Promise<boolean> {
    return (await this.call(() => this.inner.requestNewGroup(largestGroup))) ?? false
  }

  /// Delivers the groups before `groupId`, after which the publisher ends the
  /// subscription.
  async endBeforeGroup(groupId: bigint): Promise<void> {
    await this.call(() => this.inner.endBeforeGroup(groupId))
  }

  /// UNSUBSCRIBE. Reads end once the publisher stops the streams.
  async close(): Promise<void> {
    if (this.closed) {
      return
    }
    this.closed = true
    await this.call(() => this.inner.close())
  }

  private async call<T>(send: () => Promise<T>): Promise<T | undefined> {
    if (this.freed) {
      return undefined
    }
    this.calls += 1
    try {
      return await send()
    } finally {
      this.calls -= 1
      this.freeIfIdle()
    }
  }

  private freeIfIdle(): void {
    if (this.closed && this.calls === 0 && !this.freed) {
      this.freed = true
      this.inner.free()
    }
  }
}

/// Every namespace published under the prefix, those published before the
/// watch included, until the session ends or `close` is called.
export class MoqNamespaceWatch implements AsyncIterable<NamespaceEvent> {
  private ended = false
  private calls = 0
  private freed = false

  constructor(private readonly inner: WasmNamespaceWatch) {}

  async *[Symbol.asyncIterator](): AsyncIterator<NamespaceEvent> {
    for (;;) {
      const change = this.ended ? undefined : await this.call(() => this.inner.next())
      if (!change) {
        this.ended = true
        this.end()
        return
      }
      const event: NamespaceEvent = { kind: change.active ? 'announced' : 'withdrawn', namespace: change.namespace }
      change.free()
      yield event
    }
  }

  /// UNSUBSCRIBE_NAMESPACE; the iteration ends.
  async close(): Promise<void> {
    if (this.ended) {
      return
    }
    this.ended = true
    await this.call(() => this.inner.close())
  }

  private async call<T>(send: () => Promise<T>): Promise<T> {
    this.calls += 1
    try {
      return await send()
    } finally {
      this.calls -= 1
      this.end()
    }
  }

  /// The wasm object is freed once the watch has ended and no call awaits it.
  private end(): void {
    if (this.ended && this.calls === 0 && !this.freed) {
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
