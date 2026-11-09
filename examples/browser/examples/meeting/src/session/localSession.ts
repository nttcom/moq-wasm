import {
  type MoqLocation,
  type MoqNamespaceWatch,
  MoqSession,
  type MoqSubscription,
  type ReceivedObject,
  type TrackWriter
} from 'moq-wasm'
import { LocalMember } from '../types/member'
import { ChatMessage } from '../types/chat'
import { MeetingMediaController } from '../media/meetingMediaController'
import {
  TranscriptTimeline,
  parseTranscriberStatus,
  type TranscriberStatus,
  type TranscriptRecord
} from '../media/transcript'
import { connectOptions } from '../../../../utils/access'
import {
  buildNamespacePrefix,
  buildTrackNamespace,
  buildTranscriberNamespace,
  parseTrackNamespace
} from './trackNamespace'
import { TrackPublication } from './trackPublication'

interface LocalSessionOptions {
  roomName: string
  userName: string
  relayUrl: string
}

const CHAT_TRACK_NAME = 'chat'
const TRANSCRIPT_TRACK_NAME = 'transcript'
const TRANSCRIBER_STATUS_TRACK_NAME = 'status'

export enum LocalSessionState {
  Idle = 'idle',
  Connecting = 'connecting',
  Ready = 'ready',
  Disconnecting = 'disconnecting',
  Disconnected = 'disconnected'
}

type NamespaceHandler = (trackNamespace: string[]) => void

export class LocalSession {
  readonly roomName: string
  readonly relayUrl: string
  readonly localMember: LocalMember
  /// Publishes the member's media and chat and subscribes to the others' chat;
  /// the media controller's players play their media on it.
  private session: MoqSession | null = null
  private namespaceWatch: Promise<MoqNamespaceWatch> | null = null
  private readonly chatTrack = new TrackPublication<TrackWriter>()
  private readonly mediaController: MeetingMediaController
  private state: LocalSessionState = LocalSessionState.Idle
  private chatMessageHandler: ((message: ChatMessage) => void) | null = null
  private publishNamespaceHandler: NamespaceHandler | null = null
  private publishNamespaceDoneHandler: NamespaceHandler | null = null
  /// Keyed by the id that `subscribeChat` and the transcriber subscriptions return.
  private readonly subscriptions = new Map<bigint, MoqSubscription>()
  private nextSubscriptionId = 0n

  constructor({ roomName, userName, relayUrl }: LocalSessionOptions) {
    this.roomName = roomName
    this.relayUrl = relayUrl
    this.localMember = { id: userName, name: userName }
    this.mediaController = new MeetingMediaController(this.trackNamespace)
  }

  get status(): LocalSessionState {
    return this.state
  }

  private transitionToState(next: LocalSessionState) {
    this.state = next
  }

  get trackNamespace(): string[] {
    return buildTrackNamespace(this.roomName, this.localMember.name)
  }

  get trackNamespacePrefix(): string[] {
    return buildNamespacePrefix(this.roomName)
  }

  setOnPublishNamespaceHandler(handler: NamespaceHandler | null): void {
    this.publishNamespaceHandler = handler
  }

  setOnPublishNamespaceDoneHandler(handler: NamespaceHandler | null): void {
    this.publishNamespaceDoneHandler = handler
  }

  setOnChatMessageHandler(handler: ((message: ChatMessage) => void) | null): void {
    this.chatMessageHandler = handler
  }

  async initialize(): Promise<void> {
    if (this.state === LocalSessionState.Ready || this.state === LocalSessionState.Connecting) {
      return
    }

    this.transitionToState(LocalSessionState.Connecting)
    try {
      const session = await MoqSession.connect(this.relayUrl, connectOptions())
      this.session = session
      void session.closed.then((close) => {
        if (this.session === session) {
          console.info('[meeting][moqt] connection closed', close)
          this.transitionToState(LocalSessionState.Disconnected)
        }
      })
      // Published at join so a recorder can subscribe to the chat before the first message.
      // A failure only affects chat: sendChatMessage publishes again.
      this.chatTrack
        .writer(() => session.publishTrack(this.trackNamespace, CHAT_TRACK_NAME))
        .catch((error: unknown) => console.warn('[meeting] chat track publish failed', error))
      await this.mediaController.publisher.start(session)
      this.mediaController.players.connect(session)
      this.transitionToState(LocalSessionState.Ready)
    } catch (error) {
      this.session?.close()
      this.session = null
      this.transitionToState(LocalSessionState.Idle)
      throw error
    }
  }

  async disconnect(): Promise<void> {
    if (this.state === LocalSessionState.Idle || this.state === LocalSessionState.Disconnected) {
      return
    }

    this.transitionToState(LocalSessionState.Disconnecting)
    try {
      await this.unsubscribeAllActiveSubscriptions()
      await this.namespaceWatch
        ?.then((watch) => watch.close())
        .catch((error) => console.warn('[meeting] failed to unsubscribe the namespace', error))
      await this.mediaController.dispose().catch((error) => console.warn('[meeting] failed to stop the media', error))
    } finally {
      this.namespaceWatch = null
      this.chatMessageHandler = null
      this.publishNamespaceHandler = null
      this.publishNamespaceDoneHandler = null
      this.chatTrack.reset()
      const session = this.session
      this.session = null
      session?.close()
      this.transitionToState(LocalSessionState.Disconnected)
    }
  }

  private connected(action: string): MoqSession {
    if (this.state !== LocalSessionState.Ready || !this.session) {
      throw new Error(`Cannot ${action} when session state is "${this.state}"`)
    }
    return this.session
  }

  /// Calls the namespace handlers for every member and bot published in the
  /// room, those already present included. Only the first call sends
  /// SUBSCRIBE_NAMESPACE (draft-14 §9.28 rejects an overlapping one on the
  /// session); later calls wait for it.
  async subscribeNamespace(): Promise<void> {
    if (this.namespaceWatch) {
      await this.namespaceWatch
      return
    }
    const watching = this.connected('subscribe namespace').watchNamespaces(this.trackNamespacePrefix)
    this.namespaceWatch = watching
    const watch = await watching.catch((error: unknown) => {
      if (this.namespaceWatch === watching) {
        this.namespaceWatch = null
      }
      throw error
    })
    void (async () => {
      for await (const { kind, namespace } of watch) {
        console.info(
          `[meeting][moqt] ${kind === 'announced' ? 'PUBLISH_NAMESPACE' : 'PUBLISH_NAMESPACE_DONE'}`,
          namespace
        )
        const handler = kind === 'announced' ? this.publishNamespaceHandler : this.publishNamespaceDoneHandler
        handler?.(namespace)
      }
    })().catch((error: unknown) => console.warn('[meeting] namespace watch ended', error))
  }

  /// A member's chat from its next group on. `onEnded` is called when the
  /// publisher ends it with PUBLISH_DONE.
  async subscribeChat(trackNamespace: string[], onEnded: (subscribeId: bigint) => void): Promise<bigint> {
    const session = this.connected('subscribe chat')
    const sender = parseTrackNamespace(trackNamespace)?.userName ?? CHAT_TRACK_NAME
    const handle = await session.subscribe(trackNamespace, CHAT_TRACK_NAME, { filter: 'nextGroupStart' })
    const id = this.track(handle, () => onEnded(id))
    this.read(id, handle, (object) => this.receiveChat(id, sender, trackNamespace, object))
    return id
  }

  private receiveChat(subscriptionId: bigint, sender: string, trackNamespace: string[], object: ReceivedObject): void {
    // The subscription id next to the location tells a second subscription from a second delivery of one object.
    console.info('[meeting][chat] receive', {
      sender,
      subscriptionId: subscriptionId.toString(),
      groupId: object.groupId.toString(),
      subgroupId: object.subgroupId.toString(),
      objectId: object.objectId.toString()
    })
    try {
      const { text, timestamp } = decodeChatPayload(new TextDecoder().decode(object.objectPayload))
      this.chatMessageHandler?.({
        sender,
        trackNamespace: [...trackNamespace],
        groupId: object.groupId,
        text,
        timestamp,
        isLocal: false
      })
    } catch (error) {
      console.error('Failed to decode a chat message:', error)
    }
  }

  /// The transcriber's event timeline, from the current group on.
  async subscribeTranscript(onRecords: (records: TranscriptRecord[]) => void, onEnded: () => void): Promise<bigint> {
    const session = this.connected('subscribe transcript')
    const timeline = new TranscriptTimeline()
    const handle = await session.subscribeCurrentGroup(buildTranscriberNamespace(this.roomName), TRANSCRIPT_TRACK_NAME)
    const id = this.track(handle, onEnded)
    this.read(id, handle, (object) => {
      if (timeline.add(object.groupId, object.objectId, object.objectPayload, !isAtOrBefore(object, handle.largest))) {
        onRecords(timeline.records())
      }
    })
    return id
  }

  /// The transcriber's recognizer and whether it answers yet: one object per group.
  async subscribeTranscriberStatus(onStatus: (status: TranscriberStatus) => void): Promise<bigint> {
    const session = this.connected('subscribe transcriber status')
    const handle = await session.subscribeCurrentGroup(
      buildTranscriberNamespace(this.roomName),
      TRANSCRIBER_STATUS_TRACK_NAME
    )
    const id = this.track(handle, () => {})
    this.read(id, handle, ({ objectPayload }) => {
      const status = parseTranscriberStatus(objectPayload)
      if (status) {
        onStatus(status)
      }
    })
    return id
  }

  isSubscribed(subscribeId: bigint): boolean {
    return this.subscriptions.has(subscribeId)
  }

  async unsubscribe(subscribeId: bigint): Promise<void> {
    this.connected('unsubscribe')
    const handle = this.subscriptions.get(subscribeId)
    this.subscriptions.delete(subscribeId)
    await handle?.close()
  }

  /// Keeps `handle` under a new id until it is unsubscribed or the publisher ends it
  /// with PUBLISH_DONE, when `onEnded` is called.
  private track(handle: MoqSubscription, onEnded: () => void): bigint {
    const id = this.nextSubscriptionId++
    this.subscriptions.set(id, handle)
    void handle.done.then((done) => {
      if (done && this.subscriptions.get(id) === handle) {
        this.subscriptions.delete(id)
        onEnded()
      }
    })
    return id
  }

  /// Hands each object of `handle` to `onObject` while `id` stays subscribed.
  private read(id: bigint, handle: MoqSubscription, onObject: (object: ReceivedObject) => void): void {
    void (async () => {
      for await (const object of handle) {
        if (this.subscriptions.get(id) === handle) {
          onObject(object)
        }
      }
    })().catch((error: unknown) => console.warn(`[meeting] subscription ${id.toString()} read ended`, error))
  }

  private async unsubscribeAllActiveSubscriptions(): Promise<void> {
    const handles = Array.from(this.subscriptions.entries())
    for (const [subscribeId] of handles) {
      this.subscriptions.delete(subscribeId)
    }
    await Promise.all(
      handles.map(async ([subscribeId, handle]) => {
        try {
          await handle.close()
        } catch (error) {
          console.warn(`[meeting] failed to unsubscribe during disconnect (${subscribeId.toString()})`, error)
        }
      })
    )
  }

  async sendChatMessage(message: string): Promise<void> {
    if (!message.trim()) {
      return
    }
    const session = this.connected('send chat message')
    const writer = await this.chatTrack.writer(() => session.publishTrack(this.trackNamespace, CHAT_TRACK_NAME))
    await writer.writeGroup(new TextEncoder().encode(encodeChatPayload(message)))
  }

  getMediaController(): MeetingMediaController {
    return this.mediaController
  }
}

/// An object at or before `location` came from the joining FETCH, since the
/// SUBSCRIBE starts after the largest object.
function isAtOrBefore(object: ReceivedObject, location: MoqLocation | undefined): boolean {
  return (
    location !== undefined &&
    (object.groupId < location.groupId || (object.groupId === location.groupId && object.objectId <= location.objectId))
  )
}

type ChatEventRecord = {
  t?: number
  data?: { text?: string }
}

function decodeChatPayload(payload: string): { text: string; timestamp: number } {
  const fallback = { text: payload, timestamp: Date.now() }
  try {
    const parsed = JSON.parse(payload)
    const record = Array.isArray(parsed) ? parsed[parsed.length - 1] : parsed
    if (!isObject(record)) {
      return fallback
    }
    const data = isObject(record.data) ? record.data : null
    const text = typeof data?.text === 'string' && data.text.trim() ? data.text : payload
    const timestamp = typeof record.t === 'number' && Number.isFinite(record.t) ? record.t : fallback.timestamp
    return { text, timestamp }
  } catch {
    return fallback
  }
}

function encodeChatPayload(message: string): string {
  const record: ChatEventRecord = { t: Date.now(), data: { text: message } }
  return JSON.stringify([record])
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}
