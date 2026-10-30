import {
  type MoqLocation,
  type MoqNamespaceWatch,
  MoqSession,
  type MoqSubscription,
  type ReceivedObject
} from 'moq-wasm'
import type { TrackWriter } from 'moq-wasm/internal/wasm'
import { CATALOG_TRACK_NAME } from 'moq-wasm/media/catalog'
import { LocalMember } from '../types/member'
import { ChatMessage } from '../types/chat'
import { MeetingMediaController } from '../media/meetingMediaController'
import type { VideoAdaptation } from '../media/mediaSubscriber'
import { parseMeetingCatalogTracks } from '../media/meetingCatalog'
import {
  TranscriptTimeline,
  parseTranscriberStatus,
  type TranscriberStatus,
  type TranscriptRecord
} from '../media/transcript'
import type { MeetingCatalogTrack, CatalogSubscribeRole, TrackMediaConfig } from '../types/catalog'
import { readMeetingAuth } from './authToken'
import {
  buildNamespacePrefix,
  buildTrackNamespace,
  buildTranscriberNamespace,
  parseTrackNamespace
} from './trackNamespace'

interface LocalSessionOptions {
  roomName: string
  userName: string
  relayUrl: string
}

/// draft-14 §7.2: the relay sends a lower subscriber priority first, so video
/// yields the downlink to audio when the two compete.
const VIDEO_SUBSCRIBER_PRIORITY = 128
const AUDIO_SUBSCRIBER_PRIORITY = 0
const CATALOG_SUBSCRIBE_TIMEOUT_MS = 5000
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
  /// Publishes the member's media and chat and subscribes to the others.
  private session: MoqSession | null = null
  private namespaceWatch: MoqNamespaceWatch | null = null
  private chatWriter: Promise<TrackWriter> | null = null
  private readonly mediaController: MeetingMediaController
  private state: LocalSessionState = LocalSessionState.Idle
  private chatMessageHandler: ((message: ChatMessage) => void) | null = null
  private publishNamespaceHandler: NamespaceHandler | null = null
  private publishNamespaceDoneHandler: NamespaceHandler | null = null
  /// By the id `subscribe` returns, which also keys the track's media registration.
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
      const session = await MoqSession.connect(this.relayUrl, { token: readMeetingAuth().token })
      this.session = session
      void session.closed.then((close) => {
        if (this.session === session) {
          console.info('[meeting][moqt] connection closed', close)
          this.transitionToState(LocalSessionState.Disconnected)
        }
      })
      await this.mediaController.publisher.start(session)
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
        ?.close()
        .catch((error) => console.warn('[meeting] failed to unsubscribe the namespace', error))
      await this.mediaController.dispose().catch((error) => console.warn('[meeting] failed to stop the media', error))
    } finally {
      this.namespaceWatch = null
      this.chatMessageHandler = null
      this.publishNamespaceHandler = null
      this.publishNamespaceDoneHandler = null
      this.chatWriter = null
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

  /// Calls the namespace handlers for every member and bot published under
  /// `trackNamespacePrefix`, those already present included.
  async subscribeNamespace(trackNamespacePrefix: string[]): Promise<void> {
    const watch = await this.connected('subscribe namespace').watchNamespaces(trackNamespacePrefix)
    this.namespaceWatch = watch
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

  /// `onEnded` is called when the publisher ends the track with PUBLISH_DONE;
  /// its pipeline is closed by then.
  async subscribe(
    trackNamespace: string[],
    trackName: string,
    role: CatalogSubscribeRole,
    config: TrackMediaConfig | undefined,
    onEnded: (subscribeId: bigint) => void,
    adaptation?: VideoAdaptation
  ): Promise<bigint> {
    const session = this.connected('subscribe')
    const { subscriber } = this.mediaController
    const remoteUser = parseTrackNamespace(trackNamespace)?.userName ?? trackName
    const isVideo = role === 'video' || role === 'screenshare'
    // A new picture starts at the current group's keyframe, which the joining FETCH brings; a track
    // that takes over a playing picture waits for its next group instead (see `RenditionSplice`).
    // NextGroupStart would never deliver the audio of a single-stream publisher, which never starts a
    // new group (draft-14 §9.7), so audio starts after the largest object.
    const joining = isVideo && subscriber.startsPicture(remoteUser, trackName)
    const handle = joining
      ? await session.subscribeCurrentGroup(trackNamespace, trackName)
      : await session.subscribe(trackNamespace, trackName, {
          filter: role === 'chat' ? 'nextGroupStart' : 'largestObject',
          priority: isVideo ? VIDEO_SUBSCRIBER_PRIORITY : AUDIO_SUBSCRIBER_PRIORITY
        })
    const id = this.track(handle, () => onEnded(id))

    if (role === 'chat') {
      this.read(id, handle, (object) => this.receiveChat(remoteUser, trackNamespace, object))
      return id
    }
    if (role === 'audio') {
      subscriber.registerAudioTrack(remoteUser, trackName, id, config)
    } else {
      subscriber.registerVideoTrack(
        remoteUser,
        trackName,
        id,
        config,
        () => {
          this.unsubscribe(id).catch((error) => console.error('Failed to unsubscribe a replaced track:', error))
        },
        adaptation
      )
    }
    const cachedUntil = joining ? handle.largest : undefined
    this.read(id, handle, (object) => subscriber.receive(id, object.groupId, object, isAtOrBefore(object, cachedUntil)))
    return id
  }

  private receiveChat(sender: string, trackNamespace: string[], object: ReceivedObject): void {
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

  /// Moves a subscribed media role to `trackName`. A track of the same codec
  /// takes over the picture without a gap and the previous subscription ends
  /// once it has; any other track replaces it after an UNSUBSCRIBE.
  async switchTrack(
    previousId: bigint,
    trackNamespace: string[],
    trackName: string,
    role: CatalogSubscribeRole,
    config: TrackMediaConfig | undefined,
    onEnded: (subscribeId: bigint) => void,
    adaptation?: VideoAdaptation
  ): Promise<bigint> {
    if (!this.subscriptions.has(previousId) || !this.mediaController.subscriber.canReplaceVideo(previousId, config)) {
      await this.unsubscribe(previousId)
    }
    return this.subscribe(trackNamespace, trackName, role, config, onEnded, adaptation)
  }

  /// Resolves after `onTracksUpdated` has received the first catalog.
  /// `onEnded` is called when the publisher ends the catalog with PUBLISH_DONE.
  async subscribeCatalog(
    trackNamespace: string[],
    onTracksUpdated: (tracks: MeetingCatalogTrack[]) => void,
    onEnded: () => void
  ): Promise<void> {
    const session = this.connected('subscribe catalog')
    const { promise: firstCatalog, resolve: resolveFirst, reject: rejectFirst } = Promise.withResolvers<void>()
    const subscribed = async () => {
      const handle = await session.subscribeCurrentGroup(trackNamespace, CATALOG_TRACK_NAME)
      const id = this.track(handle, onEnded)
      this.read(id, handle, ({ groupId, objectPayload }) => {
        try {
          const tracks = parseMeetingCatalogTracks(new TextDecoder().decode(objectPayload))
          console.info('[meeting][catalog] apply', {
            trackNamespace,
            groupId: groupId.toString(),
            tracks: tracks.map((t) => t.role)
          })
          onTracksUpdated(tracks)
          resolveFirst()
        } catch (error) {
          console.error('Failed to parse catalog payload:', error)
          rejectFirst(error instanceof Error ? error : new Error('Failed to parse catalog payload'))
        }
      })
      return firstCatalog
    }
    let timeoutId: number | undefined
    const timedOut = new Promise<never>((_, reject) => {
      timeoutId = window.setTimeout(
        () => reject(new Error('Catalog subscribe timed out')),
        CATALOG_SUBSCRIBE_TIMEOUT_MS
      )
    })
    try {
      await Promise.race([subscribed(), timedOut])
    } finally {
      window.clearTimeout(timeoutId)
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
    this.forgetSubscription(subscribeId)
    await handle?.close()
  }

  /// Keeps `handle` under a new id until it is unsubscribed or the publisher ends it
  /// with PUBLISH_DONE, when `onEnded` is called.
  private track(handle: MoqSubscription, onEnded: () => void): bigint {
    const id = this.nextSubscriptionId++
    this.subscriptions.set(id, handle)
    void handle.done.then((done) => {
      if (done && this.subscriptions.get(id) === handle) {
        this.forgetSubscription(id)
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

  private forgetSubscription(subscribeId: bigint): void {
    this.subscriptions.delete(subscribeId)
    this.mediaController.subscriber.unregisterTrack(subscribeId)
  }

  private async unsubscribeAllActiveSubscriptions(): Promise<void> {
    const handles = Array.from(this.subscriptions.entries())
    for (const [subscribeId] of handles) {
      this.forgetSubscription(subscribeId)
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
    const chatWriter = (this.chatWriter ??= session.publishTrack(this.trackNamespace, 'chat'))
    const writer = await chatWriter.catch((error: unknown) => {
      if (this.chatWriter === chatWriter) {
        this.chatWriter = null
      }
      throw error
    })
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
